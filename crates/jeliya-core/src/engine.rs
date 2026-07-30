//! The transport-free typed engine: one typed operation in, its typed output
//! out, plus the typed push fan-out, moved to the protocol-v2 surface (#166).
//! Every transport — the WebSocket daemon, an in-process host — drives this
//! same typed implementation, so the protocol-v2 contract holds for all of
//! them by construction.
//!
//! The engine owns everything below the transport line and nothing wire-side:
//! it never sees JSON, a frame, or a method string. The codec (#164) decodes a
//! request into a typed [`Call`]; the engine resolves it into a
//! [`crate::typed::TypedCall`] and executes it against the supervisor; the
//! codec encodes the typed reply. The generation gate, the envelope, and all
//! JSON live in the codec and the host — never here.
//!
//! v2-only by construction: there is no v1 dispatch table and no compatibility
//! facade. A legacy client is refused at the codec's generation gate, before
//! any frame is parsed or any dispatch occurs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jeliya_api::{ApiError, PeerRow, Push, RoomId, SubjectState};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::error::{CoreResult, ErrorKind};
use crate::supervisor::RoomSupervisor;
use crate::typed::{self, TypedCall, TypedReply};

/// The protocol generation this engine serves. One generation at a time, no
/// dual support: v2's generation. Part of the supervision contract — an app
/// adopts a running daemon only when this matches what it was built against.
pub const PROTOCOL_VERSION: u64 = 2;

/// The minimum supported protocol generation. v2 supports exactly one.
pub const MIN_PROTOCOL_VERSION: u64 = 2;

/// The storage generation. Bumped for the clean-slate v2 state: a v1 data dir
/// is never read as v2 state.
pub const STORAGE_GENERATION: u64 = 2;

/// The engine tick for the reconcile safety net + peer-change drain (~300ms).
/// Live `Push::Event` frames arrive immediately via each room's `room_events`
/// pump, so this tick is no longer the latency path — only the reconcile that
/// a lossy broadcast cannot let drift.
const PUSH_TICK: Duration = Duration::from_millis(300);

/// This crate's own version, for hosts that report the engine's version.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Host-supplied facts the engine cannot know on its own.
pub struct EngineConfig {
    /// The actually bound port (`0` for an in-process host — unambiguous "no
    /// listener", since a bound daemon can never truthfully report 0).
    pub port: u16,
    /// The version string the host reports (`jeliyad` passes its own crate
    /// version; an in-process host passes [`CORE_VERSION`]).
    pub version: String,
    /// `daemon.stop` target; the string is the human-readable reason. The
    /// reply-first beat lives in the stop arm. `jeliyad` passes its
    /// process-shutdown channel; an in-process host passes a sender whose
    /// receiver performs real engine teardown.
    pub shutdown_tx: mpsc::Sender<String>,
}

/// The engine: a [`RoomSupervisor`] plus the typed push fan-out channel.
/// Cheap to share (`Arc`); no engine-wide lock — the supervisor guards its
/// own maps internally, never across an `.await`.
pub struct Engine {
    supervisor: Arc<RoomSupervisor>,
    /// Typed push frames, broadcast once at the send site; every subscriber
    /// forwards them to its connection. Capacity 1024; a lagged subscriber
    /// misses pushes and re-syncs via `stream.resync` (the one resync path).
    push_tx: broadcast::Sender<Push>,
    config: EngineConfig,
    /// Set once a `daemon.stop` has been accepted: a second stop is
    /// `shutdown_in_progress`, never a comfortable repeat `stopping: true`.
    stopping: Arc<AtomicBool>,
}

/// The result of executing one typed call: the reply to encode, plus the
/// server-side effect the host must honor after the reply is flushed.
pub struct Executed {
    /// The typed reply.
    pub reply: Result<TypedReply, ApiError>,
    /// When the call was `daemon.stop`, the host flushes the reply and then
    /// initiates teardown. The engine sequences the signal; the host owns the
    /// actual process/in-process shutdown.
    pub stop_after_reply: bool,
}

impl Engine {
    /// Create an engine owning a fresh supervisor over `data_dir` (created if
    /// missing, then canonicalized). Synchronous; the engine never creates a
    /// runtime, it assumes an ambient one for its spawned work.
    pub fn new(data_dir: PathBuf, loopback: bool, config: EngineConfig) -> CoreResult<Arc<Self>> {
        crate::identity::ensure_dir(&data_dir)?;
        let data_dir = data_dir.canonicalize().unwrap_or(data_dir);
        let supervisor = Arc::new(RoomSupervisor::new(data_dir, loopback)?);
        Ok(Self::with_supervisor(supervisor, config))
    }

    /// Wrap an existing supervisor. Used by hosts that need their own handle
    /// to it besides dispatch — `jeliyad`'s HTTP staging endpoints call the
    /// supervisor directly.
    #[must_use]
    pub fn with_supervisor(supervisor: Arc<RoomSupervisor>, config: EngineConfig) -> Arc<Self> {
        let (push_tx, _) = broadcast::channel(1024);
        Arc::new(Self {
            supervisor,
            push_tx,
            config,
            stopping: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The underlying supervisor (for host surfaces that bypass dispatch).
    #[must_use]
    pub fn supervisor(&self) -> &Arc<RoomSupervisor> {
        &self.supervisor
    }

    /// The resolved data directory.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        self.supervisor.data_dir()
    }

    /// The host's configured port (from [`EngineConfig`]).
    #[must_use]
    pub fn port(&self) -> u16 {
        self.config.port
    }

    /// The host's configured version string.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.config.version
    }

    /// The served limits, surfaced in the `hello` frame and `VersionInfo`.
    #[must_use]
    pub fn limits(&self) -> jeliya_api::Limits {
        typed::limits()
    }

    /// The `hello` `subject` fact: present with ids, its stated absence, or
    /// `not_ready` when the subject store cannot be read (the connection must
    /// be refused rather than invited to run `subject.ensure` against
    /// unreadable existing state).
    pub fn subject_state(&self) -> Result<SubjectState, ApiError> {
        typed::TypedSupervisor::new(&self.supervisor).subject_state()
    }

    /// Execute one typed call. This is the engine's only dispatch surface:
    /// total by construction (the codec's router already refused any `op`
    /// outside the 33, so the [`TypedCall`] always maps to exactly one output).
    pub async fn execute(&self, call: TypedCall) -> Executed {
        let stop_after_reply = matches!(call, TypedCall::DaemonStop(_));
        if stop_after_reply {
            // Exactly one teardown: a second `daemon.stop` is
            // `shutdown_in_progress`, never a comfortable repeat
            // `stopping: true`. The check-and-set is atomic so two concurrent
            // stops on two sessions cannot both sequence a shutdown.
            if self.stopping.swap(true, Ordering::SeqCst) {
                return Executed {
                    reply: Err(ApiError::ShutdownInProgress),
                    stop_after_reply: false,
                };
            }
            // Reply first, then die: the shutdown signal is delayed a beat so
            // the reply flushes to the requesting client before teardown.
            let tx = self.config.shutdown_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                let _ = tx.send("daemon.stop".to_owned()).await;
            });
        }
        let reply = typed::dispatch(&self.supervisor, call).await;
        Executed {
            reply,
            stop_after_reply,
        }
    }

    /// Subscribe to the typed push fan-out. A lagged subscriber misses frames
    /// (never re-sent) and is told to resync; `Closed` ends the connection.
    #[must_use]
    pub fn subscribe_pushes(&self) -> broadcast::Receiver<Push> {
        self.push_tx.subscribe()
    }

    /// Publish one typed push to every subscriber. The push loop calls this;
    /// hosts never construct pushes themselves.
    fn emit(&self, push: Push) {
        // No subscribers is fine — pushes are fan-out, not delivery-guaranteed.
        let _ = self.push_tx.send(push);
    }

    /// Spawn the push fan-out (per-room pump tasks + the reconcile ticker +
    /// the peer-change drain) onto the ambient tokio runtime.
    ///
    /// MUST run whenever the engine is live, even with zero push subscribers:
    /// the reconcile's `poll_new_events` is the sole maintainer of the
    /// join-bootstrap `accept_joins` window — invites stall without it.
    ///
    /// Dropping the returned handle DETACHES the loop (it runs for the
    /// engine's life); only [`PushLoopHandle::stop`] cancels the ticker, after
    /// which the pumps die on `RoomNotOpen` as rooms close.
    pub fn start_push_loop(self: &Arc<Self>) -> PushLoopHandle {
        let (cancel, cancel_rx) = watch::channel(false);
        let task = tokio::spawn(push_loop(self.clone(), cancel_rx));
        PushLoopHandle { cancel, task }
    }

    /// Close every open room (releasing its blob locks and network session).
    /// Bounded: a room whose teardown hangs must not turn shutdown into a
    /// zombie, so after 10s the caller proceeds anyway and it is noted.
    ///
    /// Returns whether EVERY room closed cleanly.
    pub async fn close_all_rooms(&self) -> bool {
        let close_all = async {
            let mut clean = true;
            for room_id in self.supervisor.open_rooms() {
                match self.supervisor.close_room(&room_id).await {
                    Ok(()) => info!("closed room {room_id}"),
                    Err(err) => {
                        warn!("could not close room {room_id} cleanly: {err}");
                        clean = false;
                    }
                }
            }
            clean
        };
        match tokio::time::timeout(Duration::from_secs(10), close_all).await {
            Ok(clean) => clean,
            Err(_) => {
                warn!("room teardown did not finish within 10s; exiting anyway");
                false
            }
        }
    }
}

/// Handle to a running push loop. Dropping it detaches the loop; call
/// [`PushLoopHandle::stop`] for explicit teardown (in-process hosts).
pub struct PushLoopHandle {
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl PushLoopHandle {
    /// Signal the ticker loop to exit. Does not await the per-room pumps:
    /// they die on `RoomNotOpen` as rooms close (`close_all_rooms`).
    pub fn stop(self) {
        let _ = self.cancel.send(true);
        drop(self.task);
    }
}

/// Resolve when [`PushLoopHandle::stop`] fires. A dropped handle closes the
/// watch channel instead; that means DETACH, so park forever rather than
/// waking the select on every poll.
async fn cancelled(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|stop| *stop).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Drive the typed push fan-out. Each open room gets a dedicated pump task
/// that awaits its node's `room_events` broadcast and pushes each newly
/// committed event as `Push::Event` the moment it commits (sub-second
/// latency). This ticker supervises those pumps, runs the reconcile safety
/// net (`poll_new_events`, which a lossy broadcast cannot let drift), and
/// drains each session's `conn_events` broadcast to push `Push::Peer` on any
/// transition.
async fn push_loop(engine: Arc<Engine>, mut cancel_rx: watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(PUSH_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let pumped: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = cancelled(&mut cancel_rx) => break,
        }
        let sup = &engine.supervisor;
        for room_id in sup.open_room_ids() {
            let room_str = room_id.to_string();
            let api_room = RoomId::new(room_str.clone());

            // Ensure a live push pump for this room.
            let fresh = pumped
                .lock()
                .expect("pumped mutex poisoned")
                .insert(room_str.clone());
            if fresh {
                let engine = engine.clone();
                let pumped = pumped.clone();
                let key = room_str.clone();
                let api_room = api_room.clone();
                let mut cancel_rx = cancel_rx.clone();
                tokio::spawn(async move {
                    loop {
                        let received = tokio::select! {
                            events = recv_typed_events(&engine, &room_id) => events,
                            () = cancelled(&mut cancel_rx) => break,
                        };
                        match received {
                            Ok(events) => {
                                for committed in events {
                                    emit_committed(&engine, &api_room, committed);
                                }
                            }
                            Err(err) if err.kind == ErrorKind::RoomNotOpen => break,
                            Err(err) => {
                                warn!("room-event pump error for {key}: {err}");
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                        }
                    }
                    pumped.lock().expect("pumped mutex poisoned").remove(&key);
                });
            }

            // Reconcile safety net: re-scan the tail so a lagged/dropped
            // broadcast event is still pushed exactly once (shared `seen`).
            match poll_typed_events(&engine, &room_id).await {
                Ok(events) => {
                    for committed in events {
                        emit_committed(&engine, &api_room, committed);
                    }
                }
                Err(err) => warn!("push reconcile failed for {room_str}: {err}"),
            }
            if sup.drain_conn_changes(&room_id) {
                for peer in typed_peer_rows(&engine, &room_id).await {
                    engine.emit(Push::Peer {
                        room_id: api_room.clone(),
                        subject_id: peer.subject_id,
                        device_id: peer.device_id,
                        link: peer.link,
                        generation: 0,
                    });
                }
            }
        }
    }
}

/// The next batch of newly committed events for one room (the primary,
/// sub-second push path), deduped against the session's `seen` set so each is
/// pushed exactly once.
async fn recv_typed_events(
    engine: &Engine,
    room_id: &iroh_rooms::room::RoomId,
) -> CoreResult<Vec<crate::supervisor::CommittedEvent>> {
    engine.supervisor.recv_room_events_typed(room_id).await
}

/// The reconcile poll: the room's not-yet-pushed events, typed, sharing the
/// same `seen` dedup as the primary path.
async fn poll_typed_events(
    engine: &Engine,
    room_id: &iroh_rooms::room::RoomId,
) -> CoreResult<Vec<crate::supervisor::CommittedEvent>> {
    engine.supervisor.poll_new_events_typed(room_id).await
}

/// Emit one newly-pushed committed event, preceded by a corrective `gap` when
/// it reordered already-served history. A late concurrent sibling interleaving
/// below the frontier shifts the ranks of events the stream already served;
/// the client cannot detect that from an event frame alone (the reordered
/// event arrives at a position it already holds), so the stream first tells it
/// to discard the shifted suffix and resync via `stream.resync` (the one
/// resync path), then delivers the event at its true rank.
fn emit_committed(
    engine: &Engine,
    api_room: &RoomId,
    committed: crate::supervisor::CommittedEvent,
) {
    if let Some(from_pos) = committed.reordered_at {
        engine.emit(Push::Gap {
            room_id: api_room.clone(),
            from_pos,
            to: jeliya_api::GapTo::Open,
            reason: jeliya_api::GapReason::Backpressure,
        });
    }
    engine.emit(Push::Event {
        room_id: api_room.clone(),
        event: committed.event,
    });
}

/// The per-device link rows for one room, for the peer-change drain.
async fn typed_peer_rows(engine: &Engine, room_id: &iroh_rooms::room::RoomId) -> Vec<PeerRow> {
    let typed = typed::TypedSupervisor::new(&engine.supervisor);
    let req = jeliya_api::RoomPeers {
        room_id: RoomId::new(room_id.to_string()),
    };
    match typed.room_peers(&req).await {
        Ok(out) => out.peers,
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_engine(dir: &TempDir) -> Arc<Engine> {
        let (shutdown_tx, _shutdown_rx) = mpsc::channel(4);
        Engine::new(
            dir.path().to_path_buf(),
            true,
            EngineConfig {
                port: 0,
                version: CORE_VERSION.to_owned(),
                shutdown_tx,
            },
        )
        .expect("engine over a temp dir")
    }

    #[tokio::test]
    async fn engine_serves_protocol_v2() {
        let dir = TempDir::new().expect("tempdir");
        let engine = test_engine(&dir);
        assert_eq!(PROTOCOL_VERSION, 2);
        assert_eq!(MIN_PROTOCOL_VERSION, 2);
        assert_eq!(STORAGE_GENERATION, 2);
        let _ = engine;
    }

    #[tokio::test]
    async fn room_list_before_identity_is_subject_absent() {
        let dir = TempDir::new().expect("tempdir");
        let engine = test_engine(&dir);
        // Validation-order step 2: with no subject, `room.list` is
        // `subject_absent` — never an empty list (the step-5 carve-out that
        // lets `room.list` enumerate left rooms does not reach step 2).
        let executed = engine
            .execute(TypedCall::RoomList(jeliya_api::RoomList {}))
            .await;
        let err = executed.reply.unwrap_err();
        assert!(matches!(err, ApiError::SubjectAbsent), "got {err:?}");
    }

    #[tokio::test]
    async fn subject_ensure_is_idempotent() {
        let dir = TempDir::new().expect("tempdir");
        let engine = test_engine(&dir);
        let first = engine
            .execute(TypedCall::SubjectEnsure(jeliya_api::SubjectEnsure {}))
            .await
            .reply
            .expect("subject.ensure succeeds");
        let TypedReply::SubjectEnsure(first) = first else {
            panic!("wrong reply");
        };
        assert!(first.created);
        let second = engine
            .execute(TypedCall::SubjectEnsure(jeliya_api::SubjectEnsure {}))
            .await
            .reply
            .expect("subject.ensure is idempotent");
        let TypedReply::SubjectEnsure(second) = second else {
            panic!("wrong reply");
        };
        assert!(!second.created);
        assert_eq!(first.subject_id, second.subject_id);
    }

    #[tokio::test]
    async fn typed_message_send_round_trips_and_pushes() {
        let dir = TempDir::new().expect("tempdir");
        let engine = test_engine(&dir);
        use jeliya_api::*;
        // Subject + room.
        engine
            .execute(TypedCall::SubjectEnsure(SubjectEnsure {}))
            .await
            .reply
            .expect("subject.ensure");
        let created = engine
            .execute(TypedCall::RoomCreate(RoomCreate {
                name: "push room".into(),
            }))
            .await
            .reply
            .expect("room.create");
        let TypedReply::RoomCreate(created) = created else {
            panic!("wrong reply");
        };
        let room_id = created.room_id.clone();

        // Activate the room, then subscribe to typed pushes before sending.
        engine
            .execute(TypedCall::RoomActivate(RoomActivate {
                room_id: room_id.clone(),
            }))
            .await
            .reply
            .expect("room.activate");
        let _push_loop = engine.clone().start_push_loop();
        let mut pushes = engine.subscribe_pushes();

        let sent = engine
            .execute(TypedCall::MessageSend(MessageSend {
                room_id: room_id.clone(),
                body: "hello typed push".into(),
            }))
            .await
            .reply
            .expect("message.send");
        let TypedReply::MessageSend(sent) = sent else {
            panic!("wrong reply");
        };
        // The second committed event in the room (after the genesis) sits at
        // dense position 1 — the rank over the canonical order, not the raw
        // lamport.
        assert_eq!(sent.pos, 1, "the reply position is the dense rank");

        // The committed event lands on the typed push fan-out as Push::Event.
        let push = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match pushes.recv().await {
                    Ok(Push::Event { room_id: r, event })
                        if r == room_id && event.kind.kind() == EventKind::Message =>
                    {
                        break event
                    }
                    Ok(_) => continue,
                    Err(e) => panic!("push recv failed: {e}"),
                }
            }
        })
        .await
        .expect("a typed Push::Event arrives within 10s");
        let EventKindContent::Message { body } = &push.kind else {
            panic!("wrong kind");
        };
        assert_eq!(body, "hello typed push");
        // Reply, push, and timeline positions are ONE dense position space:
        // the push carries exactly the position the reply served.
        assert_eq!(push.pos, sent.pos, "push and reply positions agree");

        // The timeline serves the same committed event, typed, at pos >= 1.
        let page = Page {
            cursor: Cursor::Start,
            direction: Direction::Forward,
            limit: 50,
        };
        let timeline = engine
            .execute(TypedCall::RoomTimeline(RoomTimeline {
                room_id: room_id.clone(),
                page,
            }))
            .await
            .reply
            .expect("room.timeline");
        let TypedReply::RoomTimeline(timeline) = timeline else {
            panic!("wrong reply");
        };
        assert_eq!(timeline.events.len(), 2); // room_created + message
        assert_eq!(timeline.events[0].kind.kind(), EventKind::RoomCreated);
        assert_eq!(timeline.events[0].pos, 0);
        assert_eq!(timeline.events[1].kind.kind(), EventKind::Message);
        assert_eq!(
            timeline.events[1].pos, sent.pos,
            "timeline and reply positions agree"
        );

        engine.close_all_rooms().await;
    }
}
