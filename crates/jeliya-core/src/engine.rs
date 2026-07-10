//! The transport-free engine facade: protocol dispatch (one JSON request
//! frame in, exactly one JSON response frame out, with the envelope and error
//! codes from `docs/PROTOCOL.md`) plus the push fan-out, moved verbatim from
//! the `jeliyad` daemon so every transport — the WebSocket daemon, the mobile
//! FFI shim — drives the same implementation and the golden conformance
//! corpus holds for all of them by construction.
//!
//! The engine owns everything below the transport line: the 24-method
//! dispatch table, the request/response envelope, and the room-event /
//! peers-changed push broadcast. Everything transport-specific (sockets,
//! auth tokens, portfiles, process lifecycle) stays with the host, which
//! supplies its facts through [`EngineConfig`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::error::{CoreError, CoreResult, ErrorKind};
use crate::{identity, supervisor::RoomSupervisor};

/// Major version of the protocol spoken over any transport
/// (`docs/PROTOCOL.md`). Part of the supervision contract: an app adopts a
/// running daemon only when this matches what it was built against; on
/// mismatch it must not spawn a second daemon on the same data dir, but stop
/// the old one and respawn. `jeliyad` re-exports this const so its portfile,
/// `ready` line, `/api/health`, and `daemon.status` can never drift apart.
pub const PROTOCOL_VERSION: u32 = 1;

/// The engine tick for the reconcile safety net + peer-change drain (~300ms
/// per the protocol build notes). Since issue #83 live `room.event` pushes
/// arrive immediately via each room's `room_events` pump, so this tick is no
/// longer the latency path — only the reconcile that a lossy broadcast cannot
/// let drift.
const PUSH_TICK: Duration = Duration::from_millis(300);

/// This crate's own version, for hosts that report the engine's version in
/// `daemon.status` (the FFI shim); `jeliyad` passes its own crate version.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Host-supplied facts the dispatch table cannot know on its own.
pub struct EngineConfig {
    /// `daemon.status` `port`. `jeliyad` passes the actually bound port; an
    /// in-process host passes `0` — unambiguous "no listener", since a bound
    /// daemon can never truthfully report 0.
    pub port: u16,
    /// `daemon.status` `version`. `jeliyad` passes its own crate version
    /// (byte-identical output); an in-process host passes [`CORE_VERSION`].
    pub version: String,
    /// `daemon.shutdown` target; the string is the human-readable reason.
    /// The 150ms reply-first beat lives in the dispatch arm. `jeliyad` passes
    /// its process-shutdown channel; an in-process host passes a sender whose
    /// receiver performs real engine teardown ({shutting_down:true} must
    /// always be followed by actual teardown).
    pub shutdown_tx: mpsc::Sender<String>,
}

/// The engine: an [`RoomSupervisor`] plus the dispatch table and the push
/// fan-out channel. Cheap to share (`Arc`); no engine-wide lock — the
/// supervisor guards its own maps internally, never across an `.await`.
pub struct Engine {
    supervisor: Arc<RoomSupervisor>,
    /// Pre-serialized push frames (`{"push":…,"data":…}`), serialized once at
    /// the send site; every subscriber forwards them verbatim. Capacity 1024;
    /// a lagged subscriber just misses pushes and re-syncs via
    /// request/response.
    push_tx: broadcast::Sender<String>,
    config: EngineConfig,
}

impl Engine {
    /// Create an engine owning a fresh supervisor over `data_dir`
    /// (created if missing, then canonicalized so `daemon.status` paths and
    /// cross-process identity checks compare like with like regardless of how
    /// the caller spelled the path — mirrors `jeliyad` startup). Synchronous;
    /// the engine never creates a runtime, it assumes an ambient one for its
    /// spawned work.
    pub fn new(data_dir: PathBuf, loopback: bool, config: EngineConfig) -> CoreResult<Arc<Self>> {
        identity::ensure_dir(&data_dir)?;
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

    /// Handle one raw text frame; always returns a serialized response
    /// envelope (`{id, ok:true, result}` or `{id, ok:false, error}`).
    pub async fn handle_frame(&self, raw: &str) -> String {
        let parsed: Result<Value, _> = serde_json::from_str(raw);
        let (id, method, params) = match parsed {
            Ok(Value::Object(mut obj)) => {
                let id = obj.remove("id").unwrap_or(Value::Null);
                let method = obj.get("method").and_then(Value::as_str).map(str::to_owned);
                let params = obj.remove("params").unwrap_or_else(|| json!({}));
                match method {
                    Some(method) => (id, method, params),
                    None => {
                        return envelope_err(
                            id,
                            &CoreError::invalid("request must carry a string \"method\""),
                        )
                    }
                }
            }
            _ => {
                return envelope_err(
                    Value::Null,
                    &CoreError::invalid("request must be a JSON object {id, method, params}"),
                )
            }
        };

        match self.dispatch(&method, params).await {
            Ok(result) => json!({ "id": id, "ok": true, "result": result }).to_string(),
            Err(err) => envelope_err(id, &err),
        }
    }

    /// The envelope-free seam: one protocol method in, its `result` object
    /// (or a protocol-coded error) out.
    pub async fn dispatch(&self, method: &str, raw_params: Value) -> CoreResult<Value> {
        // The supervisor is a plain `Arc` — no engine-wide lock is taken here, so a
        // slow request (a `file.fetch` against an offline provider, a `pipe.connect`
        // busy-wait) runs on its own without head-of-line blocking any other client
        // or the push loop. The supervisor guards its own session map internally,
        // only for the span of a map lookup, never across a network await.
        let sup = &self.supervisor;
        match method {
            // ---- Daemon & identity -------------------------------------------
            "daemon.status" => Ok(daemon_status(sup, &self.config)),
            "daemon.shutdown" => {
                // Reply first, then die: the shutdown signal is delayed a beat so
                // this response flushes to the requesting client before teardown.
                let tx = self.config.shutdown_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    let _ = tx.send("daemon.shutdown RPC".to_owned()).await;
                });
                Ok(json!({ "shutting_down": true }))
            }
            "identity.create" => {
                let profile = identity::create(self.data_dir())?;
                Ok(json!({
                    "identity_id": profile.identity_id,
                    "device_id": profile.device_id,
                }))
            }

            // ---- Rooms --------------------------------------------------------
            "room.create" => {
                let p: CreateRoomParams = params(raw_params)?;
                let room_id = sup.create_room(&p.name)?;
                Ok(json!({ "room_id": room_id }))
            }
            "room.list" => Ok(json!({ "rooms": sup.list_rooms().await? })),
            "room.open" => {
                let p: OpenRoomParams = params(raw_params)?;
                sup.open_room(&p.room_id, p.peers.as_deref().unwrap_or(&[]))
                    .await
            }
            "room.close" => {
                let p: RoomIdParams = params(raw_params)?;
                sup.close_room(&p.room_id).await?;
                Ok(json!({}))
            }
            "room.leave" => {
                let p: RoomIdParams = params(raw_params)?;
                let event_id = sup.leave_room(&p.room_id).await?;
                Ok(json!({ "event_id": event_id }))
            }
            "room.timeline" => {
                let p: TimelineParams = params(raw_params)?;
                Ok(json!({ "events": sup.timeline(&p.room_id, p.limit).await? }))
            }
            "room.members" => {
                let p: RoomIdParams = params(raw_params)?;
                Ok(json!({ "members": sup.members(&p.room_id).await? }))
            }
            "invite.create" => {
                let p: InviteParams = params(raw_params)?;
                let expiry = expiry_spec(p.expiry)?;
                let ticket = sup
                    .create_invite(&p.room_id, &p.identity_id, &p.role, expiry.as_deref())
                    .await?;
                Ok(json!({ "ticket": ticket }))
            }
            "room.join" => {
                let p: JoinParams = params(raw_params)?;
                let room_id = sup
                    .join_room(
                        &p.ticket,
                        p.name.as_deref(),
                        p.peers.as_deref().unwrap_or(&[]),
                    )
                    .await?;
                Ok(json!({ "room_id": room_id }))
            }

            // ---- Messages & agent status ---------------------------------------
            "message.send" => {
                let p: SendParams = params(raw_params)?;
                let event_id = sup.send_message(&p.room_id, &p.body).await?;
                Ok(json!({ "event_id": event_id }))
            }
            "status.post" => {
                let p: StatusPostParams = params(raw_params)?;
                let event_id = sup
                    .post_status(
                        &p.room_id,
                        &p.label,
                        p.message.as_deref(),
                        p.progress,
                        p.artifacts.as_deref().unwrap_or(&[]),
                    )
                    .await?;
                Ok(json!({ "event_id": event_id }))
            }

            // ---- Files ----------------------------------------------------------
            "file.share" => {
                let p: FileShareParams = params(raw_params)?;
                sup.share_file(&p.room_id, &p.path, p.name.as_deref(), p.mime.as_deref())
                    .await
            }
            "file.list" => {
                let p: RoomIdParams = params(raw_params)?;
                Ok(json!({ "files": sup.list_files(&p.room_id).await? }))
            }
            "file.fetch" => {
                let p: FileFetchParams = params(raw_params)?;
                sup.fetch_file(&p.room_id, &p.file_id, p.save_dir.as_deref())
                    .await
            }

            // ---- Pipes ----------------------------------------------------------
            "pipe.expose" => {
                let p: PipeExposeParams = params(raw_params)?;
                sup.pipe_expose(&p.room_id, &p.target, &p.peer_identity)
                    .await
            }
            "pipe.list" => {
                let p: RoomIdParams = params(raw_params)?;
                Ok(json!({ "pipes": sup.pipe_list(&p.room_id)? }))
            }
            "pipe.connect" => {
                let p: PipeIdParams = params(raw_params)?;
                let local_addr = sup.pipe_connect(&p.room_id, &p.pipe_id).await?;
                Ok(json!({ "local_addr": local_addr }))
            }
            "pipe.close" => {
                let p: PipeIdParams = params(raw_params)?;
                sup.pipe_close(&p.room_id, &p.pipe_id).await
            }

            // ---- Agents (fleet reads) --------------------------------------------
            "agents.fleet" => sup.agents_fleet().await,
            "agent.history" => {
                let p: AgentHistoryParams = params(raw_params)?;
                sup.agent_history(&p.room_id, &p.identity_id, p.limit)
            }

            // ---- Peers ----------------------------------------------------------
            "peers.status" => {
                let p: RoomIdParams = params(raw_params)?;
                Ok(json!({ "peers": sup.peers_status(&p.room_id).await? }))
            }

            other => Err(CoreError::invalid(format!("unknown method {other:?}"))
                .with_hint("see docs/PROTOCOL.md for the method list")),
        }
    }

    /// Subscribe to the push fan-out: pre-serialized `{"push":"room.event",…}`
    /// / `{"push":"peers.changed",…}` frames, forwarded verbatim by every
    /// transport. A lagged subscriber misses frames (never re-sent) and
    /// re-syncs via the request/response surface.
    #[must_use]
    pub fn subscribe_pushes(&self) -> broadcast::Receiver<String> {
        self.push_tx.subscribe()
    }

    /// Spawn the push fan-out (per-room pump tasks + the reconcile ticker +
    /// the peer-change drain) onto the ambient tokio runtime.
    ///
    /// MUST run whenever the engine is live, even with zero push subscribers:
    /// the reconcile's `poll_new_events` is the sole maintainer of the
    /// join-bootstrap `accept_joins` window — invites stall without it.
    ///
    /// Dropping the returned handle DETACHES the loop (it runs for the
    /// engine's life — `jeliyad`'s run-forever behavior); only
    /// [`PushLoopHandle::stop`] cancels the ticker, after which the pumps die
    /// on `RoomNotOpen` as rooms close.
    pub fn start_push_loop(self: &Arc<Self>) -> PushLoopHandle {
        let (cancel, cancel_rx) = watch::channel(false);
        let task = tokio::spawn(push_loop(self.clone(), cancel_rx));
        PushLoopHandle { cancel, task }
    }

    /// Close every open room (releasing its blob locks and network session).
    /// Bounded: a room whose teardown hangs must not turn shutdown into a
    /// zombie, so after 10s the caller proceeds anyway and it is noted.
    ///
    /// Returns whether EVERY room closed cleanly. On `false`, the unclosed
    /// rooms never ran `Node::shutdown` — the only thing that releases a
    /// room's exclusive on-disk blob lock — so their stores may stay locked
    /// until the OS process exits. `jeliyad` exits right after, so the OS
    /// reaps them; an in-process host outlives this call and must report the
    /// unclean close instead of claiming success.
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

/// Drive the room-event push fan-out (issue #83).
///
/// Each open room gets a dedicated pump task that awaits its node's
/// `room_events` broadcast and pushes each new validated event as `room.event`
/// the moment it commits (sub-second latency, no hot tail poll). This ticker
/// (~300ms) supervises those pumps, runs the reconcile safety net
/// (`poll_new_events`, which a lossy broadcast cannot let drift and which keeps
/// the join-bootstrap window tied to live state), and drains each session's
/// `conn_events` broadcast to push `peers.changed` with truthful direct/relay
/// path info on any transition. The pump and the reconcile share the
/// supervisor's per-room `seen` set, so every event is pushed exactly once.
async fn push_loop(engine: Arc<Engine>, mut cancel_rx: watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(PUSH_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Rooms with a live per-room `room_events` pump task. Shared with the pumps
    // so a pump deregisters itself on exit (room.close), letting a later re-open
    // re-spawn a fresh pump.
    let pumped: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = cancelled(&mut cancel_rx) => break,
        }
        let sup = &engine.supervisor;
        for room_id in sup.open_room_ids() {
            let room_str = room_id.to_string();

            // Ensure a live push pump for this room.
            let fresh = pumped
                .lock()
                .expect("pumped mutex poisoned")
                .insert(room_str.clone());
            if fresh {
                let engine = engine.clone();
                let pumped = pumped.clone();
                let key = room_str.clone();
                // The pump watches the same cancel signal as the ticker: it
                // normally dies on `RoomNotOpen` as its room closes, but a
                // room whose close hangs or fails would otherwise park the
                // pump forever, pinning the whole Engine through this task's
                // `Arc` — fatal for an in-process host, which has no process
                // exit to reap it. (A DROPPED handle still detaches: the
                // closed watch channel parks `cancelled` forever.)
                let mut cancel_rx = cancel_rx.clone();
                tokio::spawn(async move {
                    loop {
                        let received = tokio::select! {
                            events = engine.supervisor.recv_room_events(&room_id) => events,
                            () = cancelled(&mut cancel_rx) => break,
                        };
                        match received {
                            Ok(events) => {
                                for event in events {
                                    let frame = json!({
                                        "push": "room.event",
                                        "data": { "room_id": key, "event": event },
                                    });
                                    let _ = engine.push_tx.send(frame.to_string());
                                }
                            }
                            // The room closed: stop pumping and deregister so a
                            // later re-open re-spawns a fresh pump.
                            Err(err) if err.kind == ErrorKind::RoomNotOpen => break,
                            // A transient read error: the reconcile poll still
                            // covers pushes; back off briefly, then keep pumping.
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
            match sup.poll_new_events(&room_id).await {
                Ok(events) => {
                    for event in events {
                        let frame = json!({
                            "push": "room.event",
                            "data": { "room_id": room_str, "event": event },
                        });
                        let _ = engine.push_tx.send(frame.to_string());
                    }
                }
                Err(err) => warn!("push reconcile failed for {room_str}: {err}"),
            }
            if sup.drain_conn_changes(&room_id) {
                if let Ok(peers) = sup.peers_status(&room_str).await {
                    let frame = json!({
                        "push": "peers.changed",
                        "data": { "room_id": room_str, "peers": peers },
                    });
                    let _ = engine.push_tx.send(frame.to_string());
                }
            }
        }
    }
}

fn envelope_err(id: Value, err: &CoreError) -> String {
    json!({
        "id": id,
        "ok": false,
        "error": {
            "code": err.kind.code(),
            "message": err.message,
            "hint": err.hint,
        },
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Params shapes
// ---------------------------------------------------------------------------

fn params<T: for<'de> Deserialize<'de>>(value: Value) -> CoreResult<T> {
    serde_json::from_value(value).map_err(|e| CoreError::invalid(format!("invalid params: {e}")))
}

#[derive(Deserialize)]
struct RoomIdParams {
    room_id: String,
}

#[derive(Deserialize)]
struct CreateRoomParams {
    name: String,
}

#[derive(Deserialize)]
struct OpenRoomParams {
    room_id: String,
    /// Optional dial hints (`"<endpoint_id>@<ip:port>"`) merged into the
    /// room's persisted hint set — loopback mode has no discovery.
    peers: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct TimelineParams {
    room_id: String,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct InviteParams {
    room_id: String,
    identity_id: String,
    role: String,
    expiry: Option<Value>,
}

#[derive(Deserialize)]
struct JoinParams {
    ticket: String,
    name: Option<String>,
    peers: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct SendParams {
    room_id: String,
    body: String,
}

#[derive(Deserialize)]
struct StatusPostParams {
    room_id: String,
    label: String,
    message: Option<String>,
    progress: Option<u64>,
    artifacts: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct FileShareParams {
    room_id: String,
    path: String,
    name: Option<String>,
    mime: Option<String>,
}

#[derive(Deserialize)]
struct FileFetchParams {
    room_id: String,
    file_id: String,
    save_dir: Option<String>,
}

#[derive(Deserialize)]
struct AgentHistoryParams {
    room_id: String,
    identity_id: String,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct PipeExposeParams {
    room_id: String,
    target: String,
    peer_identity: String,
}

#[derive(Deserialize)]
struct PipeIdParams {
    room_id: String,
    pipe_id: String,
}

/// Accept `"24h"` / `"3600"` (string spec) or a bare number of seconds.
fn expiry_spec(value: Option<Value>) -> CoreResult<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(|secs| Some(format!("{secs}s")))
            .ok_or_else(|| CoreError::invalid("expiry must be a positive integer of seconds")),
        Some(other) => Err(CoreError::invalid(format!(
            "expiry must be a string like \"24h\" or a number of seconds, got {other}"
        ))),
    }
}

fn daemon_status(sup: &RoomSupervisor, config: &EngineConfig) -> Value {
    let identity = identity::load_profile(sup.data_dir())
        .ok()
        .flatten()
        .map(|p| json!({ "identity_id": p.identity_id, "device_id": p.device_id }));
    json!({
        "version": config.version,
        "protocol": PROTOCOL_VERSION,
        "pid": std::process::id(),
        "port": config.port,
        "data_dir": sup.data_dir().display().to_string(),
        "mode": sup.mode(),
        "identity": identity,
        "endpoint": sup.status_endpoint(),
        "rooms_open": sup.open_rooms(),
    })
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

    fn parse(frame: &str) -> Value {
        serde_json::from_str(frame).expect("response envelope is JSON")
    }

    #[tokio::test]
    async fn non_object_frame_errors_with_null_id() {
        let dir = TempDir::new().expect("tempdir");
        let engine = test_engine(&dir);
        for raw in ["[1,2,3]", "\"hello\"", "not json at all"] {
            let reply = parse(&engine.handle_frame(raw).await);
            assert_eq!(reply["id"], Value::Null, "frame {raw:?}");
            assert_eq!(reply["ok"], json!(false), "frame {raw:?}");
            assert_eq!(
                reply["error"]["code"],
                json!("invalid_params"),
                "frame {raw:?}"
            );
        }
    }

    #[tokio::test]
    async fn missing_method_echoes_the_id() {
        let dir = TempDir::new().expect("tempdir");
        let engine = test_engine(&dir);
        let reply = parse(&engine.handle_frame(r#"{"id":42,"params":{}}"#).await);
        assert_eq!(reply["id"], json!(42));
        assert_eq!(reply["ok"], json!(false));
        assert_eq!(reply["error"]["code"], json!("invalid_params"));
    }

    #[tokio::test]
    async fn unknown_method_carries_the_protocol_hint() {
        let dir = TempDir::new().expect("tempdir");
        let engine = test_engine(&dir);
        let reply = parse(&engine.handle_frame(r#"{"id":1,"method":"no.such"}"#).await);
        assert_eq!(reply["ok"], json!(false));
        assert_eq!(reply["error"]["code"], json!("invalid_params"));
        assert_eq!(
            reply["error"]["hint"],
            json!("see docs/PROTOCOL.md for the method list")
        );
    }

    #[tokio::test]
    async fn daemon_status_reports_port_zero_truthfully() {
        let dir = TempDir::new().expect("tempdir");
        let engine = test_engine(&dir);
        let status = engine
            .dispatch("daemon.status", json!({}))
            .await
            .expect("daemon.status succeeds");
        assert_eq!(status["port"], json!(0));
        assert_eq!(status["protocol"], json!(PROTOCOL_VERSION));
        assert_eq!(status["pid"], json!(std::process::id()));
        assert_eq!(status["version"], json!(CORE_VERSION));
        assert_eq!(status["mode"], json!("loopback"));
        assert_eq!(
            status["data_dir"],
            json!(engine.data_dir().display().to_string())
        );
        // No identity was created and no room is open.
        assert_eq!(status["identity"], Value::Null);
        assert_eq!(status["endpoint"], Value::Null);
        assert_eq!(status["rooms_open"], json!([]));
    }
}
