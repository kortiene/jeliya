//! `DirectEngineActor` — the `FfiHost` port that owns the live [`Engine`] (§5.2).
//!
//! A single struct owning everything the in-process engine needs, torn down as
//! one unit. It is a direct port of `crates/jeliya-ffi/src/host.rs` minus the
//! Dart `SendPort`/`nativePort`/C-ABI machinery and the retired
//! `Engine::handle_frame(String)` seam: the engine is called only through the
//! typed `Engine::execute_with`, and pushes are observed only through the typed
//! `subscribe_pushes()`. No bytes cross any transport.
//!
//! The **serialized dispatch task** is the WAL-race guard the FFI host
//! documents, not a style choice: the supervisor opens the per-room SQLite store
//! per call, and concurrent first-ever opens on a fresh `rooms.db` race the WAL
//! transition into "database is locked". Running `execute_with` strictly one at
//! a time is exactly the "Calls execute serially" acceptance criterion.

use std::path::Path;

use jeliya_api::{OpId, Push, RequestId};
use jeliya_core::engine::{Engine, EngineConfig, PushLoopHandle, CORE_VERSION};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::backend::RawJson;
use crate::direct::bridge;
use crate::direct::ownership::{self, OwnerToken, OwnershipError};
use crate::kernel::transport::WireReply;

/// The fixed, non-secret session principal for the in-process engine (§5.3,
/// OQ-5). DirectClient has no authenticated wire principal and no token; a stable
/// per-session string makes envelope `op_id` dedup/idempotency behave within the
/// session exactly as it does on the wire, and it is never a credential.
pub(crate) const PRINCIPAL_KEY: &str = "direct";

/// One request handed to the serialized dispatch task: the wire correlation id,
/// the typed call already resolved by the [`bridge`], and the caller's envelope
/// `op_id` (forwarded verbatim so in-session dedup matches the wire).
pub(crate) struct EngineRequest {
    /// The reply-correlation id the kernel awaits.
    pub(crate) id: RequestId,
    /// The typed call to execute.
    pub(crate) call: jeliya_core::typed::TypedCall,
    /// The caller's envelope dedup key.
    pub(crate) op_id: Option<OpId>,
}

/// One reply the dispatch task returns for the driver loop to feed to the core.
pub(crate) struct EngineReply {
    /// The correlation id this answers.
    pub(crate) id: RequestId,
    /// The erased reply body (typed output text, or a typed daemon error).
    pub(crate) result: WireReply,
    /// Set when the engine sequenced a `daemon.stop`: the reply is delivered,
    /// then the driver runs the same teardown as an explicit stop.
    pub(crate) stop_after_reply: bool,
}

/// The receive halves the driver loop selects on: engine replies, live pushes,
/// and local push-overflow (`Lagged`) signals.
pub(crate) struct EngineChannels {
    /// Serialized replies from the dispatch task.
    pub(crate) reply_rx: mpsc::Receiver<EngineReply>,
    /// Live typed pushes, forwarded for the engine's whole life.
    pub(crate) push_rx: mpsc::Receiver<Push>,
    /// The count of pushes the forward task missed to a lagged broadcast — the
    /// local-overflow signal the reconciler resyncs on.
    pub(crate) lag_rx: mpsc::Receiver<u64>,
}

/// The outcome of a [`DirectEngineActor::teardown`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct TeardownOutcome {
    /// `true` when every room closed within the engine's bounded close budget.
    /// `false` means rooms stayed open past it — their `rooms.db`/blob locks may
    /// remain held until process exit, so a re-open over the same dir would then
    /// fail. Mirrors `FfiHost::teardown`'s `0`/`1` done code.
    pub(crate) clean: bool,
}

/// Why opening the in-process engine failed. Either outcome maps to a terminal
/// gate refusal in the driver (§5.3) — there is nothing to back off to.
#[derive(Debug)]
pub(crate) enum OpenError {
    /// A live owner already controls this canonical data directory.
    Ownership(OwnershipError),
    /// `Engine::new` failed to open the new-generation storage.
    Engine,
}

/// Everything owned on behalf of the live engine; torn down as one unit.
pub(crate) struct DirectEngineActor {
    engine: std::sync::Arc<Engine>,
    /// Feeds the serialized dispatch task; the only sender. Bounded (§9): the
    /// kernel's in-flight throttle is the true admission bound above it, so this
    /// small hand-off buffer never overflows in correct operation.
    requests_tx: mpsc::Sender<EngineRequest>,
    push_loop: Option<PushLoopHandle>,
    dispatch: JoinHandle<()>,
    push_forward: JoinHandle<()>,
    shutdown_watch: JoinHandle<()>,
    /// The ownership grant; its `Drop` releases the data dir on teardown.
    _owner: OwnerToken,
}

impl DirectEngineActor {
    /// Open the engine over `data_dir` **once**: acquire ownership (fail-closed),
    /// construct `Engine::new`, start the push loop immediately (before any
    /// subscriber, for join-bootstrap maintenance), and spawn the serialized
    /// dispatch and push-forward tasks. This is the "serialized first-open"; the
    /// registry guarantees one `Engine::new` per canonical dir at a time and the
    /// single dispatch task serializes the racing first room-store opens.
    pub(crate) fn open(
        data_dir: &Path,
        request_channel_depth: usize,
        push_channel_depth: usize,
    ) -> Result<(Self, EngineChannels), OpenError> {
        let owner = ownership::acquire(data_dir).map_err(OpenError::Ownership)?;

        let (shutdown_tx, shutdown_rx) = mpsc::channel::<String>(4);
        let config = EngineConfig {
            // 0 = unambiguous "no listener": a bound daemon can never report 0.
            port: 0,
            version: CORE_VERSION.to_owned(),
            shutdown_tx,
        };
        let engine = match Engine::new(data_dir.to_path_buf(), true, config) {
            Ok(engine) => engine,
            Err(_) => return Err(OpenError::Engine),
        };

        // Immediately, even with zero subscribers: the push loop's reconcile
        // poll is the sole maintainer of the join-bootstrap accept_joins window
        // (AC "always-run push-loop join maintenance").
        let push_loop = engine.start_push_loop();

        let (requests_tx, requests_rx) = mpsc::channel::<EngineRequest>(request_channel_depth);
        let (reply_tx, reply_rx) = mpsc::channel::<EngineReply>(request_channel_depth);
        let (push_tx, push_rx) = mpsc::channel::<Push>(push_channel_depth);
        let (lag_tx, lag_rx) = mpsc::channel::<u64>(4);

        let dispatch = spawn_dispatch(engine.clone(), requests_rx, reply_tx);
        let push_forward = spawn_push_forward(engine.subscribe_pushes(), push_tx, lag_tx);
        // The engine's `daemon.stop` sequences a delayed signal here; the driver
        // loop already tears down on the reply's `stop_after_reply`, so this task
        // only drains the signal (single teardown entry point, Risk 6). It ends
        // on its own once the engine — the sole sender — drops.
        let shutdown_watch = tokio::spawn(async move {
            let mut shutdown_rx = shutdown_rx;
            while shutdown_rx.recv().await.is_some() {}
        });

        Ok((
            Self {
                engine,
                requests_tx,
                push_loop: Some(push_loop),
                dispatch,
                push_forward,
                shutdown_watch,
                _owner: owner,
            },
            EngineChannels {
                reply_rx,
                push_rx,
                lag_rx,
            },
        ))
    }

    /// A sender clone the driver loop enqueues [`EngineRequest`]s on.
    pub(crate) fn requests(&self) -> mpsc::Sender<EngineRequest> {
        self.requests_tx.clone()
    }

    /// Deterministic, bounded teardown (§5.2), in order:
    ///
    /// 1. `push_loop.stop()` — no new room pumps spawn while rooms close;
    /// 2. drop `requests_tx` and `dispatch.await` — every already-accepted call
    ///    finishes and returns its reply (an accepted call must not lose its
    ///    reply), and the task exits **before** the close snapshots the open-room
    ///    set, so a mid-flight `room.activate` cannot slip past the close;
    /// 3. `close_all_rooms().await` — internally bounded to 10 s;
    /// 4. abort the push-forward and shutdown-drain tasks;
    /// 5. drop the engine and release the ownership token.
    ///
    /// The returned [`TeardownOutcome`] reports whether every room closed within
    /// the budget. Idempotent token release: the token drops exactly once here.
    pub(crate) async fn teardown(mut self) -> TeardownOutcome {
        // 1. Ticker first, so no new room pumps spawn while rooms close.
        if let Some(push_loop) = self.push_loop.take() {
            push_loop.stop();
        }
        // 2. Dispatch drained: dropping the queue's sender lets the task finish
        // every accepted request and exit before the close.
        drop(self.requests_tx);
        let _ = self.dispatch.await;
        // 3. Internally bounded (10 s): a hung room must not zombify shutdown.
        let clean = self.engine.close_all_rooms().await;
        // 4. Push forwarding and the shutdown drain have nothing left to do.
        self.push_forward.abort();
        self.shutdown_watch.abort();
        // 5. The engine's strong ref drops here; every cleanly-closed room
        // already released its `rooms.db` handles and blob locks. The ownership
        // token releases on the returned struct's drop.
        drop(self.engine);
        TeardownOutcome { clean }
    }
}

/// Dispatch request frames **strictly one at a time**, returning each reply on
/// the reply channel — the direct twin of `serve_ws`'s per-connection loop and
/// of `FfiHost::spawn_dispatch`. The serialization is the WAL-race guard, not a
/// style choice.
fn spawn_dispatch(
    engine: std::sync::Arc<Engine>,
    mut requests_rx: mpsc::Receiver<EngineRequest>,
    reply_tx: mpsc::Sender<EngineReply>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(request) = requests_rx.recv().await {
            let executed = engine
                .execute_with(request.call, request.op_id, PRINCIPAL_KEY)
                .await;
            let result = match executed.reply {
                Ok(reply) => match bridge::encode_reply(&reply) {
                    Ok(text) => WireReply::Ok(RawJson::from_string(text)),
                    Err(err) => WireReply::Err(err),
                },
                Err(err) => WireReply::Err(err),
            };
            // A dropped receiver (teardown dropped the driver loop's reply_rx)
            // makes this fail fast rather than block the drain — the calls were
            // already settled by the kernel's stop, so the reply is moot.
            if reply_tx
                .send(EngineReply {
                    id: request.id,
                    result,
                    stop_after_reply: executed.stop_after_reply,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    })
}

/// Forward every typed push to the driver loop for the engine's life. A lagged
/// broadcast is **not** dropped silently: it surfaces the missed count so the
/// reconciler re-baselines (`ResyncReason::LocalOverflow`), mirroring the WS
/// drain policy. `Closed` means the engine dropped, so the task ends itself.
fn spawn_push_forward(
    mut pushes: broadcast::Receiver<Push>,
    push_tx: mpsc::Sender<Push>,
    lag_tx: mpsc::Sender<u64>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match pushes.recv().await {
                Ok(push) => {
                    if push_tx.send(push).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    // Surface local overflow; a full lag channel already carries
                    // an unconsumed resync signal, so dropping this one is safe.
                    let _ = lag_tx.try_send(dropped);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}
