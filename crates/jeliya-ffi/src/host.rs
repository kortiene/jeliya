//! The process-singleton engine host behind the `jeliya_engine_*` exports:
//! one [`Engine`] at a time over one data dir, driven by this crate's own
//! multi-thread tokio runtime (in-process there is no `#[tokio::main]` daemon
//! to provide one, and FFI entry points arrive on Flutter's UI thread — every
//! engine future is `spawn`ed, never `block_on`).
//!
//! State discipline: `HOST` (`Mutex<Option<FfiHost>>`) is the ONLY instance
//! guard — no fd-lock protects the FFI data dir the way `jeliyad`'s portfile
//! dance protects the daemon's. The runtime itself is `OnceLock`-forever:
//! engine stop/start cycles (Android lifecycle) reuse it, because runtime
//! teardown from within one of its own tasks would deadlock.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use jeliya_core::engine::{Engine, EngineConfig, PushLoopHandle, CORE_VERSION};
use jeliya_core::identity;
use tokio::runtime::Runtime;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::dart_api;

/// Everything owned on behalf of the live engine; torn down as one unit.
struct FfiHost {
    engine: Arc<Engine>,
    /// The one Dart `SendPort.nativePort` carrying every reply envelope and
    /// push frame (mirroring the single WS text-frame stream); Dart
    /// correlates replies by envelope id. Shared with the dispatch and drain
    /// tasks, which load it per post — a hot-restart adopt rebinds them in
    /// place by storing the new port.
    frames_port: Arc<AtomicI64>,
    /// The engine's network mode at construction; an adopt with a differing
    /// flag is refused rather than silently serving the wrong mode.
    loopback: bool,
    /// Feeds the dispatch task; requests are queued here in arrival order.
    requests_tx: mpsc::UnboundedSender<String>,
    push_loop: PushLoopHandle,
    dispatch: JoinHandle<()>,
    frames_drain: JoinHandle<()>,
    shutdown_watch: JoinHandle<()>,
}

static HOST: Mutex<Option<FfiHost>> = Mutex::new(None);
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .thread_name("jeliya-engine")
            .enable_all()
            .build()
            // Contained by the catch_unwind at every export.
            .expect("jeliya-ffi: tokio runtime construction failed")
    })
}

fn lock_host() -> MutexGuard<'static, Option<FfiHost>> {
    // A panic while the lock was held is already contained at the export
    // boundary; the Option is only ever replaced whole, so the state a
    // poisoned guard exposes is coherent — recover it.
    HOST.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Construct-or-rebind (`jeliya_engine_start`). See the export doc for the
/// return-code contract.
pub(crate) fn start(data_dir: &str, loopback: bool, frames_port: dart_api::Dart_Port_DL) -> i32 {
    let mut host = lock_host();

    if let Some(live) = host.as_ref() {
        if !same_data_dir(live.engine.data_dir(), Path::new(data_dir)) {
            return crate::JELIYA_FFI_ERR_DATA_DIR_MISMATCH;
        }
        // Adoption must not silently change the engine's configuration: a
        // hot restart re-runs start() with the SAME flags, so a differing
        // `loopback` means the caller wants an engine this live one is not
        // (its mode would contradict what `daemon.status` reports).
        if live.loopback != loopback {
            return crate::JELIYA_FFI_ERR_CONFIG_MISMATCH;
        }
        // Hot restart: the Dart side lost its ports but the engine (and its
        // rooms.db / blob locks) survived in-process. Adopt it — rebind the
        // frames port; the dispatch and drain tasks load it per post, so
        // even replies already in flight reach the NEW port from here on.
        // Adoption cannot distinguish a hot restart (old isolate gone) from
        // a second coexisting client, which is why one process must hold at
        // most one live FfiClient (see the export doc).
        live.frames_port.store(frames_port, Ordering::Release);
        return crate::JELIYA_FFI_ADOPTED;
    }

    let rt = runtime();
    // Engine construction is sync, but start_push_loop (and the daemon.shutdown
    // dispatch arm later) tokio::spawn onto the ambient runtime.
    let _ambient = rt.enter();
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<String>(4);
    let config = EngineConfig {
        // 0 = unambiguous "no listener": a bound daemon can never report 0.
        port: 0,
        version: CORE_VERSION.to_owned(),
        shutdown_tx,
    };
    let engine = match Engine::new(PathBuf::from(data_dir), loopback, config) {
        Ok(engine) => engine,
        Err(_) => return crate::JELIYA_FFI_ERR_ENGINE,
    };
    let frames_port = Arc::new(AtomicI64::new(frames_port));
    // Immediately, even with zero subscribers: the push loop's reconcile poll
    // is the sole maintainer of the join-bootstrap accept_joins window.
    let push_loop = engine.start_push_loop();
    let (requests_tx, requests_rx) = mpsc::unbounded_channel::<String>();
    let dispatch = spawn_dispatch(&engine, &frames_port, requests_rx);
    let frames_drain = spawn_frames_drain(&engine, &frames_port);
    let shutdown_watch = rt.spawn(watch_shutdown(shutdown_rx));
    *host = Some(FfiHost {
        engine,
        frames_port,
        loopback,
        requests_tx,
        push_loop,
        dispatch,
        frames_drain,
        shutdown_watch,
    });
    crate::JELIYA_FFI_OK
}

/// Submit one request frame (`jeliya_engine_request`). Non-blocking for the
/// caller: the frame is queued for the dispatch task, which posts the reply
/// envelope to the frames port when its turn completes.
pub(crate) fn request(frame: String) -> i32 {
    let host = lock_host();
    let Some(live) = host.as_ref() else {
        return crate::JELIYA_FFI_ERR_NOT_STARTED;
    };
    // Unreachable while the host slot is live (teardown empties the slot
    // before the dispatch task can drop its receiver), kept as a guard.
    if live.requests_tx.send(frame).is_err() {
        return crate::JELIYA_FFI_ERR_NOT_STARTED;
    }
    crate::JELIYA_FFI_OK
}

/// Bounded teardown (`jeliya_engine_stop`): returns immediately, posts one
/// completion int to `done_port` when the engine is down — `0` for a clean
/// teardown, `1` when rooms remained open past the close budget (their
/// on-disk stores may stay locked until the process exits).
pub(crate) fn stop(done_port: dart_api::Dart_Port_DL) -> i32 {
    let Some(live) = lock_host().take() else {
        return crate::JELIYA_FFI_ERR_NOT_STARTED;
    };
    // The host slot is empty from here, so a new start() may race the tail of
    // this teardown — which is why Dart must await done_port before starting
    // an engine over a different data dir.
    runtime().spawn(async move {
        let code = teardown(live).await;
        let _ = dart_api::post_int(done_port, code);
    });
    crate::JELIYA_FFI_OK
}

/// `daemon.shutdown` honesty: the dispatch arm replies `{shutting_down:true}`
/// and then signals this receiver, which must follow through with the same
/// real teardown `jeliya_engine_stop` performs.
async fn watch_shutdown(mut shutdown_rx: mpsc::Receiver<String>) {
    if shutdown_rx.recv().await.is_some() {
        // Bind before awaiting: the guard temporary must not live across the
        // teardown await (MutexGuard is !Send).
        let taken = lock_host().take();
        if let Some(live) = taken {
            let _ = teardown(live).await;
        }
    }
    // None: the engine (the only sender) was already dropped by an explicit
    // jeliya_engine_stop — nothing left to tear down.
}

/// Tear one host down. Returns the done code: `0` when every room closed
/// cleanly, `1` when rooms remained open past the close budget — those
/// rooms never ran `Node::shutdown`, the only thing that releases a room's
/// exclusive on-disk blob lock, so their stores may stay locked for the
/// rest of the OS process (dropping the engine cannot release them; a
/// re-`start` over the same data dir will fail to open the affected rooms).
async fn teardown(host: FfiHost) -> i64 {
    let FfiHost {
        engine,
        frames_port: _,
        loopback: _,
        requests_tx,
        push_loop,
        dispatch,
        frames_drain,
        shutdown_watch,
    } = host;
    // Ticker first, so no new room pumps spawn while rooms close.
    push_loop.stop();
    // Dispatch second, DRAINED: dropping the queue's sender lets the task
    // finish every already-accepted frame — an accepted request must never
    // silently lose its reply envelope (on the daemon.shutdown path the
    // frames port is still open and a caller may be awaiting it) — and exit
    // BEFORE close_all_rooms snapshots the open-room set, so a mid-flight
    // room.open cannot slip a room past the close.
    drop(requests_tx);
    let _ = dispatch.await;
    // Internally bounded (10s): a hung room must not zombify app shutdown.
    let clean = engine.close_all_rooms().await;
    frames_drain.abort();
    // Harmless self-abort on the daemon.shutdown path (this IS the watch
    // task): abort only lands at an await point and none remain below.
    shutdown_watch.abort();
    // The host's strong ref; the push-loop pumps hold clones until their
    // cancel lands. Every room close_all_rooms closed already released its
    // rooms.db handles and blob locks via Node::shutdown; an uncloseable
    // room's stay held, which is what the return code reports.
    drop(engine);
    i64::from(!clean)
}

/// Dispatch request frames STRICTLY one at a time, posting each reply to the
/// current frames port — the FFI twin of `serve_ws`'s per-connection loop,
/// where one client's requests never overlap. The serialization is
/// load-bearing, not a style choice: the supervisor opens the SQLite event
/// store per call, and concurrent first-ever opens on a fresh `rooms.db`
/// race the WAL transition into "database is locked" despite the busy
/// timeout.
fn spawn_dispatch(
    engine: &Arc<Engine>,
    frames_port: &Arc<AtomicI64>,
    mut requests_rx: mpsc::UnboundedReceiver<String>,
) -> JoinHandle<()> {
    let engine = engine.clone();
    let frames_port = frames_port.clone();
    runtime().spawn(async move {
        while let Some(frame) = requests_rx.recv().await {
            let reply = engine.handle_frame(&frame).await;
            // False (port closed, e.g. mid-hot-restart) drops the reply; the
            // Dart side already failed its in-flight calls when it closed
            // the port, so nothing waits on this.
            let _ = dart_api::post_bytes(frames_port.load(Ordering::Acquire), reply.as_bytes());
        }
    })
}

/// Forward every push frame to the Dart frames port for the engine's life.
/// Lagged skips mirror the WS drain policy: a lagged subscriber misses
/// pushes (never re-sent) and re-syncs via request/response; Closed means
/// the engine dropped, so the drain ends itself even un-aborted.
fn spawn_frames_drain(engine: &Arc<Engine>, frames_port: &Arc<AtomicI64>) -> JoinHandle<()> {
    let mut pushes = engine.subscribe_pushes();
    let frames_port = frames_port.clone();
    runtime().spawn(async move {
        loop {
            match pushes.recv().await {
                Ok(frame) => {
                    let _ =
                        dart_api::post_bytes(frames_port.load(Ordering::Acquire), frame.as_bytes());
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// Whether `requested` names the live engine's data dir, applying the same
/// normalization as `Engine::new` (ensure + canonicalize, fall back to the
/// spelled path) so "the same dir spelled differently" adopts instead of
/// being refused as a mismatch.
fn same_data_dir(live: &Path, requested: &Path) -> bool {
    let _ = identity::ensure_dir(requested);
    let requested = requested
        .canonicalize()
        .unwrap_or_else(|_| requested.to_path_buf());
    live == requested
}
