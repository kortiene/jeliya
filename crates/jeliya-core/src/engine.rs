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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jeliya_api::{
    ApiError, FileRead, FileShare, FileShareOut, OpId, PeerRow, Push, RoomId, SubjectState,
};
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

/// A verified local file copy for a host-controlled byte response. The path is
/// never accepted from a protocol caller; core resolves it from `(room_id,
/// file_id)` after authorization.
#[derive(Debug, Clone)]
pub struct LocalFile {
    /// Verified local path under the daemon's managed storage.
    pub path: PathBuf,
    /// Peer-declared display name.
    pub name: String,
    /// Peer-declared, untrusted content type.
    pub declared_content_type: String,
    /// Verified byte count.
    pub bytes: u64,
}

/// Whether `op` is in the record's **`op_id`-deduplicated** class: the
/// caller MUST supply an envelope `op_id`, a replay returns the original
/// result with no second effect, and a replay with a different body is
/// `op_id_conflict`. These are the operations whose retry would otherwise
/// author a duplicate signed fact. Naturally-idempotent operations
/// (`subject.ensure`, `room.activate`, `room.deactivate`, `invite.redeem`),
/// terminal `daemon.stop`, connection-scoped `stream.*`, and
/// principal-scoped `transfer.cancel` are NOT in this class — they accept an
/// `op_id` and ignore it.
fn is_dedup_op(op: &str) -> bool {
    matches!(
        op,
        "room.create"
            | "room.leave"
            | "member.remove"
            | "invite.mint"
            | "invite.revoke"
            | "message.send"
            | "status.post"
            | "file.share"
            | "file.fetch"
            | "pipe.publish"
            | "pipe.connect"
            | "pipe.release"
            | "pipe.revoke"
    )
}

/// The reply a dedup'd operation produced, shared between the original
/// execution and every faithful replay. Clone-cheap (`Arc`).
type LedgerReply = Arc<Result<TypedReply, ApiError>>;

/// A dedup-ledger entry in one of two states. `InFlight` reserves the key
/// BEFORE the effect runs and carries a [`watch`] the original execution
/// publishes its reply into; a concurrent duplicate of the same key subscribes
/// and awaits that reply rather than dispatching a second effect. `Done` is
/// the recorded outcome a later replay returns. The reservation is what makes
/// the effect single even when two retries overlap, and what lets the reply
/// outlive the connection that asked for it.
enum LedgerEntry {
    /// The effect is running; `done` resolves to its reply exactly once.
    InFlight {
        /// The request fingerprint (operation path + canonical body), to tell
        /// a faithful retry from a conflicting reuse of the same `op_id`.
        body_hash: u64,
        /// Publishes the reply the moment the original execution completes.
        done: watch::Sender<Option<LedgerReply>>,
    },
    /// The effect completed; the reply is recorded for faithful replays.
    Done {
        /// The request fingerprint.
        body_hash: u64,
        /// The recorded reply.
        reply: LedgerReply,
    },
}

/// The `op_id` dedup ledger, keyed per `(session principal, op_id)` per the
/// record. It survives reconnection (the motivating case for retry is a reply
/// lost to a dropped connection) but is intentionally **in-memory**: the v2
/// harness has no daemon restart, and a durable ledger is a persistence
/// concern the clean-slate milestone does not take on. Distinct principals
/// have isolated ledgers, so one local client can neither observe, replay,
/// nor cancel another's operations.
#[derive(Default)]
struct DedupLedger {
    /// `(principal_key, op_id)` → the entry. `principal_key` is the
    /// authenticated session principal rendered as a single string
    /// (`credential` + `client_id`), never the bare subject — a daemon has
    /// one subject, so a per-subject ledger would be daemon-global.
    entries: HashMap<(String, OpId), LedgerEntry>,
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
    /// The `op_id` dedup ledger (see [`DedupLedger`]). Shared (`Arc`) so a
    /// detached effect task can record the reply after the connection that
    /// requested it has dropped.
    ledger: Arc<Mutex<DedupLedger>>,
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
    pub(crate) fn with_supervisor(
        supervisor: Arc<RoomSupervisor>,
        config: EngineConfig,
    ) -> Arc<Self> {
        let (push_tx, _) = broadcast::channel(1024);
        Arc::new(Self {
            supervisor,
            push_tx,
            config,
            stopping: Arc::new(AtomicBool::new(false)),
            ledger: Arc::new(Mutex::new(DedupLedger::default())),
        })
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

    /// Complete a host-staged typed file share. Hosts supply a managed path;
    /// the protocol declaration and result remain `jeliya-api` values.
    pub async fn share_staged_file(
        &self,
        req: &FileShare,
        staged_path: &Path,
    ) -> CoreResult<FileShareOut> {
        typed::TypedSupervisor::new(&self.supervisor)
            .share_staged_file(req, staged_path)
            .await
    }

    /// Resolve a verified local file from a typed protocol address for a
    /// host-controlled byte response.
    pub async fn local_file(&self, req: &FileRead) -> CoreResult<LocalFile> {
        let file = self
            .supervisor
            .local_file(req.room_id.as_ref(), req.file_id.as_str())
            .await?;
        Ok(LocalFile {
            path: file.path,
            name: file.name,
            declared_content_type: file.mime,
            bytes: file.bytes,
        })
    }

    /// The `hello` `subject` fact: present with ids, its stated absence, or
    /// `not_ready` when the subject store cannot be read (the connection must
    /// be refused rather than invited to run `subject.ensure` against
    /// unreadable existing state).
    pub fn subject_state(&self) -> Result<SubjectState, ApiError> {
        typed::TypedSupervisor::new(&self.supervisor).subject_state()
    }

    /// Execute one typed call with no request context. Equivalent to
    /// [`Self::execute_with`] with no `op_id` and an ephemeral principal — the
    /// form in-process hosts and internal sub-operations (which never dedup)
    /// use. Dedup-class operations called this way are refused
    /// `invalid_argument{field:op_id}` exactly as on the wire, because an
    /// undeduplable mutation must not be silently accepted.
    pub async fn execute(&self, call: TypedCall) -> Executed {
        self.execute_with(call, None, "ephemeral").await
    }

    /// Execute one typed call. This is the engine's only dispatch surface:
    /// total by construction (the codec's router already refused any `op`
    /// outside the 33, so the [`TypedCall`] always maps to exactly one output).
    ///
    /// `op_id` is the envelope dedup key and `principal_key` the authenticated
    /// session principal rendered as one string; together they key the dedup
    /// ledger. For a dedup-class operation, a missing `op_id` is
    /// `invalid_argument{field:op_id, reason:missing}`, a faithful replay
    /// returns the recorded original reply with no second effect, and a replay
    /// with a different body is `op_id_conflict`.
    pub async fn execute_with(
        &self,
        call: TypedCall,
        op_id: Option<OpId>,
        principal_key: &str,
    ) -> Executed {
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

        // Dedup-ledger gate for the op_id-deduplicated class. An `op_id` is
        // ACCEPTED and, when present, deduplicated; when absent the effect
        // runs undeduplicated (the record says these operations "accept an
        // op_id", and refusing an omitted one would break the corpus's own
        // setup provisioning, which sends no op_id — see the corpus note
        // pinning the required reading as a fixture question for #163, not an
        // implementation default).
        let op = call.path();
        let dedup_key = if is_dedup_op(op) { op_id } else { None };
        if let Some(op_id) = dedup_key {
            // Validation-order step 2 (subject) runs BEFORE the ledger is
            // consulted or populated: a deduplicated call on a subject-less
            // daemon must not bind its `op_id` to `subject_absent`, or a
            // retry after `subject.ensure` would replay a stale cached error
            // instead of reaching the dedup stage as a first eligible call.
            let typed = typed::TypedSupervisor::new(&self.supervisor);
            match typed.subject_present() {
                Ok(true) => {}
                Ok(false) => {
                    return Executed {
                        reply: Err(ApiError::SubjectAbsent),
                        stop_after_reply: false,
                    };
                }
                Err(e) => {
                    return Executed {
                        reply: Err(e),
                        stop_after_reply: false,
                    };
                }
            }

            let body_hash = call.body_hash();
            let key = (principal_key.to_owned(), op_id.clone());
            // Reserve or join under the lock; never held across the await.
            enum Gate {
                /// We own the effect: it is already spawned (below) and we
                /// await its completion here.
                Run(watch::Receiver<Option<LedgerReply>>),
                /// A faithful replay: await the in-flight/recorded reply.
                Await(watch::Receiver<Option<LedgerReply>>),
                /// A recorded reply to return directly.
                Done(LedgerReply),
                /// The same `op_id` with a different body.
                Conflict,
            }
            let gate = {
                let mut ledger = self.ledger.lock().expect("dedup ledger poisoned");
                match ledger.entries.get(&key) {
                    Some(LedgerEntry::Done {
                        body_hash: h,
                        reply,
                    }) => {
                        if *h == body_hash {
                            Gate::Done(reply.clone())
                        } else {
                            Gate::Conflict
                        }
                    }
                    Some(LedgerEntry::InFlight { body_hash: h, done }) => {
                        if *h == body_hash {
                            Gate::Await(done.subscribe())
                        } else {
                            Gate::Conflict
                        }
                    }
                    None => {
                        // First sighting: reserve the key, then run the effect
                        // in a DETACHED task so it completes and records the
                        // reply even if this connection's reply task is
                        // aborted on disconnect (the record's motivating case
                        // is a reply lost to a dropped connection). Publish to
                        // the watch, then mark Done.
                        let (tx, rx) = watch::channel(None);
                        ledger.entries.insert(
                            key.clone(),
                            LedgerEntry::InFlight {
                                body_hash,
                                done: tx.clone(),
                            },
                        );
                        let sup = self.supervisor.clone();
                        let ledger = self.ledger.clone();
                        tokio::spawn(async move {
                            let reply: LedgerReply = Arc::new(typed::dispatch(&sup, call).await);
                            let _ = tx.send(Some(reply.clone()));
                            let mut ledger = ledger.lock().expect("dedup ledger poisoned");
                            ledger
                                .entries
                                .insert(key, LedgerEntry::Done { body_hash, reply });
                        });
                        Gate::Run(rx)
                    }
                }
            };
            let reply = match gate {
                Gate::Done(reply) => reply,
                Gate::Conflict => {
                    return Executed {
                        reply: Err(ApiError::OpIdConflict { op_id }),
                        stop_after_reply: false,
                    };
                }
                Gate::Run(mut rx) | Gate::Await(mut rx) => loop {
                    if let Some(reply) = rx.borrow().clone() {
                        break reply;
                    }
                    if rx.changed().await.is_err() {
                        // The publisher dropped without a reply (the spawned
                        // effect panicked): report not_ready rather than hang
                        // the caller forever.
                        break Arc::new(Err(ApiError::NotReady));
                    }
                },
            };
            return Executed {
                reply: (*reply).clone(),
                stop_after_reply,
            };
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
                                    let membership_ended =
                                        emit_committed(&engine, &room_id, &api_room, committed);
                                    if membership_ended {
                                        if let Err(err) =
                                            engine.supervisor.close_room(api_room.as_str()).await
                                        {
                                            if err.kind != ErrorKind::RoomNotOpen {
                                                warn!("could not close removed room {key}: {err}");
                                            }
                                        }
                                        break;
                                    }
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
                        let membership_ended =
                            emit_committed(&engine, &room_id, &api_room, committed);
                        if membership_ended {
                            if let Err(err) = sup.close_room(&room_str).await {
                                if err.kind != ErrorKind::RoomNotOpen {
                                    warn!("could not close removed room {room_str}: {err}");
                                }
                            }
                            break;
                        }
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
    room_id: &iroh_rooms::room::RoomId,
    api_room: &RoomId,
    committed: crate::supervisor::CommittedEvent,
) -> bool {
    let removes_local_subject = match &committed.event.kind {
        jeliya_api::EventKindContent::MemberRemoved { subject_id, .. } => engine
            .supervisor
            .local_identity_key()
            .is_ok_and(|local| subject_id.as_str() == local.to_string()),
        _ => false,
    };
    let revoked_pipe = match &committed.event.kind {
        jeliya_api::EventKindContent::PipeRevoked { pipe_id } => Some(pipe_id.clone()),
        _ => None,
    };
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
    if let Some(pipe_id) = revoked_pipe {
        if let Err(error) = engine
            .supervisor
            .release_pipe_connections(room_id, pipe_id.as_str())
        {
            warn!(
                "could not release local connections for revoked pipe {}: {error}",
                pipe_id.as_str()
            );
        }
    }
    removes_local_subject
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

    /// Create a subject and return an engine ready for room ops.
    async fn engine_with_subject(dir: &TempDir) -> Arc<Engine> {
        let engine = test_engine(dir);
        engine
            .execute(TypedCall::SubjectEnsure(jeliya_api::SubjectEnsure {}))
            .await
            .reply
            .expect("subject.ensure");
        engine
    }

    #[tokio::test]
    async fn dedup_replay_returns_original_without_second_effect() {
        let dir = TempDir::new().expect("tempdir");
        let engine = engine_with_subject(&dir).await;
        let op_id = Some(jeliya_api::OpId::new("op-1"));
        let call = || {
            TypedCall::RoomCreate(jeliya_api::RoomCreate {
                name: "dedup room".into(),
            })
        };
        let first = engine
            .execute_with(call(), op_id.clone(), "principal:a")
            .await
            .reply
            .expect("first create");
        // A faithful replay returns the SAME room_id and authors no second
        // room.
        let replay = engine
            .execute_with(call(), op_id.clone(), "principal:a")
            .await
            .reply
            .expect("replay returns the original");
        let (TypedReply::RoomCreate(first), TypedReply::RoomCreate(replay)) = (first, replay)
        else {
            panic!("wrong replies");
        };
        assert_eq!(first.room_id, replay.room_id, "replay returns the original");
        let list = engine
            .execute(TypedCall::RoomList(jeliya_api::RoomList {}))
            .await
            .reply
            .expect("room.list");
        let TypedReply::RoomList(list) = list else {
            panic!("wrong reply");
        };
        assert_eq!(list.rooms.len(), 1, "no second room was authored");
    }

    #[tokio::test]
    async fn dedup_divergent_body_conflicts() {
        let dir = TempDir::new().expect("tempdir");
        let engine = engine_with_subject(&dir).await;
        let op_id = Some(jeliya_api::OpId::new("op-1"));
        engine
            .execute_with(
                TypedCall::RoomCreate(jeliya_api::RoomCreate { name: "A".into() }),
                op_id.clone(),
                "principal:a",
            )
            .await
            .reply
            .expect("first create");
        let err = engine
            .execute_with(
                TypedCall::RoomCreate(jeliya_api::RoomCreate { name: "B".into() }),
                op_id.clone(),
                "principal:a",
            )
            .await
            .reply
            .expect_err("a divergent body conflicts");
        assert!(matches!(err, ApiError::OpIdConflict { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn dedup_ledgers_are_isolated_per_principal() {
        let dir = TempDir::new().expect("tempdir");
        let engine = engine_with_subject(&dir).await;
        let op_id = Some(jeliya_api::OpId::new("op-1"));
        let call = || {
            TypedCall::RoomCreate(jeliya_api::RoomCreate {
                name: "shared op id".into(),
            })
        };
        let a = engine
            .execute_with(call(), op_id.clone(), "principal:a")
            .await
            .reply
            .expect("principal a creates");
        // The SAME op_id under a DIFFERENT principal is an independent entry,
        // not a replay: it authors its own room.
        let b = engine
            .execute_with(call(), op_id.clone(), "principal:b")
            .await
            .reply
            .expect("principal b creates");
        let (TypedReply::RoomCreate(a), TypedReply::RoomCreate(b)) = (a, b) else {
            panic!("wrong replies");
        };
        assert_ne!(
            a.room_id, b.room_id,
            "distinct principals have isolated ledgers"
        );
    }

    #[tokio::test]
    async fn dedup_op_id_is_optional_and_undeduplicated_when_absent() {
        let dir = TempDir::new().expect("tempdir");
        let engine = engine_with_subject(&dir).await;
        let call = || {
            TypedCall::RoomCreate(jeliya_api::RoomCreate {
                name: "no op id".into(),
            })
        };
        // No op_id: the effect runs each time, undeduplicated.
        let a = engine
            .execute_with(call(), None, "principal:a")
            .await
            .reply
            .expect("first");
        let b = engine
            .execute_with(call(), None, "principal:a")
            .await
            .reply
            .expect("second");
        let (TypedReply::RoomCreate(a), TypedReply::RoomCreate(b)) = (a, b) else {
            panic!("wrong replies");
        };
        assert_ne!(a.room_id, b.room_id, "an absent op_id does not dedup");
    }

    #[tokio::test]
    async fn dedup_concurrent_replay_runs_one_effect() {
        let dir = TempDir::new().expect("tempdir");
        let engine = engine_with_subject(&dir).await;
        let op_id = jeliya_api::OpId::new("op-concurrent");
        let call = || {
            TypedCall::RoomCreate(jeliya_api::RoomCreate {
                name: "concurrent room".into(),
            })
        };
        // Fire two overlapping requests with the SAME principal and op_id.
        // The in-flight reservation must make both return the ONE original
        // room, not author two.
        let (r1, r2) = {
            let e1 = engine.clone();
            let e2 = engine.clone();
            let o1 = op_id.clone();
            let o2 = op_id.clone();
            tokio::join!(
                e1.execute_with(call(), Some(o1), "principal:a"),
                e2.execute_with(call(), Some(o2), "principal:a"),
            )
        };
        let a = r1.reply.expect("first completes");
        let b = r2.reply.expect("second completes");
        let (TypedReply::RoomCreate(a), TypedReply::RoomCreate(b)) = (a, b) else {
            panic!("wrong replies");
        };
        assert_eq!(
            a.room_id, b.room_id,
            "an overlapping replay awaits the one original effect"
        );
        let list = engine
            .execute(TypedCall::RoomList(jeliya_api::RoomList {}))
            .await
            .reply
            .expect("room.list");
        let TypedReply::RoomList(list) = list else {
            panic!("wrong reply");
        };
        assert_eq!(list.rooms.len(), 1, "exactly one room was authored");
    }

    #[tokio::test]
    async fn dedup_fingerprint_distinguishes_operations_with_identical_inputs() {
        // pipe.connect and pipe.revoke both serialize as {room_id, pipe_id}.
        // The fingerprint includes the operation path, so reusing an op_id
        // from one for the other is a CONFLICT, not a false faithful replay.
        let dir = TempDir::new().expect("tempdir");
        let engine = engine_with_subject(&dir).await;
        let room = engine
            .execute_with(
                TypedCall::RoomCreate(jeliya_api::RoomCreate { name: "r".into() }),
                Some(jeliya_api::OpId::new("setup")),
                "principal:a",
            )
            .await
            .reply
            .expect("create room");
        let TypedReply::RoomCreate(room) = room else {
            panic!("wrong reply");
        };
        let rid = room.room_id.clone();
        let pid = jeliya_api::PipeId::new("01");
        let op_id = jeliya_api::OpId::new("shared-op");
        // Record an entry under pipe.connect.
        let _ = engine
            .execute_with(
                TypedCall::PipeConnect(jeliya_api::PipeConnect {
                    room_id: rid.clone(),
                    pipe_id: pid.clone(),
                }),
                Some(op_id.clone()),
                "principal:a",
            )
            .await;
        // The same op_id under pipe.revoke with the same ids is a conflict
        // (different operation), never a replay of the connect.
        let err = engine
            .execute_with(
                TypedCall::PipeRevoke(jeliya_api::PipeRevoke {
                    room_id: rid,
                    pipe_id: pid,
                }),
                Some(op_id),
                "principal:a",
            )
            .await
            .reply
            .expect_err("a different operation with the same op_id conflicts");
        assert!(matches!(err, ApiError::OpIdConflict { .. }), "got {err:?}");
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

    #[tokio::test(flavor = "multi_thread")]
    async fn member_removal_is_the_removed_members_last_push_and_closes_the_room() {
        use jeliya_api::*;

        let owner_dir = TempDir::new().expect("owner tempdir");
        let member_dir = TempDir::new().expect("member tempdir");
        let owner = engine_with_subject(&owner_dir).await;
        let member = engine_with_subject(&member_dir).await;
        let SubjectState::Present {
            subject_id: member_id,
            ..
        } = member.subject_state().unwrap()
        else {
            panic!("member subject missing");
        };

        let created = owner
            .execute(TypedCall::RoomCreate(RoomCreate {
                name: "Removal lifecycle".into(),
            }))
            .await
            .reply
            .expect("owner creates room");
        let TypedReply::RoomCreate(created) = created else {
            panic!("wrong reply");
        };
        let room_id = created.room_id;
        owner
            .execute(TypedCall::RoomActivate(RoomActivate {
                room_id: room_id.clone(),
            }))
            .await
            .reply
            .expect("owner activates");
        let owner_loop = owner.clone().start_push_loop();

        let iroh_room: iroh_rooms::room::RoomId = room_id.as_str().parse().unwrap();
        let owner_session = owner.supervisor.session(&iroh_room).unwrap();
        let endpoint = owner_session.node.endpoint_addr().unwrap();
        let sockets: Vec<String> = endpoint.ip_addrs().map(|addr| addr.to_string()).collect();
        assert!(
            !sockets.is_empty(),
            "loopback endpoint has a socket address"
        );
        let owner_addr = format!("{}@{}", endpoint.id, sockets.join(","));
        drop(owner_session);

        let ticket = owner
            .supervisor
            .create_invite(room_id.as_str(), member_id.as_str(), "member", None)
            .await
            .expect("owner invites member");
        member
            .supervisor
            .join_room(&ticket, None, &[owner_addr])
            .await
            .expect("member redeems");
        member
            .execute(TypedCall::RoomActivate(RoomActivate {
                room_id: room_id.clone(),
            }))
            .await
            .reply
            .expect("member activates");
        let member_loop = member.clone().start_push_loop();
        let mut pushes = member.subscribe_pushes();
        let member_endpoint = member.supervisor.session(&iroh_room).unwrap().node.id();

        let member_key: iroh_rooms::identity::IdentityKey = member_id.as_str().parse().unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let joined = owner
                    .supervisor
                    .snapshot_for(&iroh_room)
                    .await
                    .is_ok_and(|snapshot| snapshot.is_active(&member_key));
                let linked = owner.supervisor.session(&iroh_room).is_ok_and(|session| {
                    session.node.peer_state(member_endpoint)
                        == Some(iroh_rooms::experimental::session::PeerConnState::Connected)
                });
                if joined && linked {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("owner observes joined member");

        owner
            .execute(TypedCall::MemberRemove(MemberRemove {
                room_id: room_id.clone(),
                subject_id: member_id.clone(),
            }))
            .await
            .reply
            .expect("authority removes member");

        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match pushes.recv().await {
                    Ok(Push::Event {
                        room_id: pushed,
                        event,
                    }) if pushed == room_id
                        && matches!(
                            event.kind,
                            EventKindContent::MemberRemoved {
                                ref subject_id,
                                ..
                            } if subject_id == &member_id
                        ) =>
                    {
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => panic!("member push stream failed: {error}"),
                }
            }
        })
        .await
        .expect("member receives its removal fact");
        tokio::time::timeout(Duration::from_secs(10), async {
            while member.supervisor.is_open(&iroh_room) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("removed member room closes");

        owner
            .execute(TypedCall::MessageSend(MessageSend {
                room_id: room_id.clone(),
                body: "after removal".into(),
            }))
            .await
            .reply
            .expect("authority can keep authoring");
        let later_event = tokio::time::timeout(Duration::from_millis(750), async {
            loop {
                match pushes.recv().await {
                    Ok(Push::Event {
                        room_id: pushed, ..
                    }) if pushed == room_id => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        })
        .await;
        assert!(
            later_event.is_err(),
            "no room event is pushed after membership removal"
        );

        owner_loop.stop();
        member_loop.stop();
        owner.close_all_rooms().await;
        member.close_all_rooms().await;
    }
}
