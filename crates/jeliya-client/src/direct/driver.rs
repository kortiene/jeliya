//! `DirectDriver` — the never-reconnecting transport binding (§5.3).
//!
//! This is the generic driver runtime (§5.1) in its DirectClient shape: a single
//! async loop that owns the concrete driver, feeds the sans-IO kernel [`Core`]
//! its inputs, and performs the `Action`s the core emits. It lives in
//! `src/direct/` — never `src/kernel/**` — so every `tokio`/clock detail stays
//! out of the boundary-scanned sans-IO core.
//!
//! DirectClient is a **degenerate** driver: it dials **once** (open the engine),
//! and because there is no transport to lose it **never** produces `Interrupted`
//! and **never** dials again. The observable [`State`](crate::event::State)
//! therefore only ever moves `Idle → Connecting → Ready → Stopping → Stopped`
//! (or `→ Failed` from a failed first-open). "Pretending DirectClient reconnects"
//! is a non-goal, made structural here.

use std::path::PathBuf;
use std::sync::Weak;
use std::time::{Duration, Instant};

use jeliya_api::{ApiError, Push, RequestId};
use tokio::sync::watch;

use crate::direct::actor::{DirectEngineActor, EngineReply, EngineRequest};
use crate::direct::bridge;
use crate::direct::runtime::DirectRuntime;
use crate::kernel::core::Input;
use crate::kernel::timing::{Tick, TimerId};
use crate::kernel::transport::{Inbound, WireFrame, WireReply};

/// One side-effect the core asked the driver to perform, forwarded from the
/// (synchronous, lock-held) action apply to the (async) driver loop. Every
/// variant's volume is bounded by the kernel's own queue/in-flight limits, so
/// the carrying channel never overflows in correct operation.
pub(crate) enum DriverMsg {
    /// Route, resolve, and dispatch one request to the engine.
    Send(WireFrame),
    /// Arm the per-call deadline timer to fire at `at`.
    ArmTimer {
        /// The timer identity.
        id: TimerId,
        /// The logical instant it fires.
        at: Tick,
    },
    /// Cancel a previously-armed timer.
    CancelTimer(TimerId),
    /// Open the engine (the one and only dial), identified by `token`.
    Dial {
        /// The dial attempt's token, echoed on the outcome.
        token: u64,
    },
    /// Total stop: tear the engine down and await it.
    CancelDial,
}

/// The async loop that binds the `DirectDriver` to the kernel core. Constructed
/// by [`connect_direct`](super::connect_direct) and spawned onto the ambient
/// tokio runtime; it exits when every [`ClientHandle`](crate::ClientHandle) that
/// shares its backend is dropped (the driver-message channel closes), tearing
/// down any live engine on the way out.
pub(crate) struct DirectDriverTask {
    /// The kernel-backed runtime this loop feeds inputs to. A `Weak` so the loop
    /// does not keep the backend alive: when the last handle drops, the runtime
    /// drops and the loop's own message channel closes, ending the loop.
    runtime: Weak<DirectRuntime>,
    data_dir: PathBuf,
    request_channel_depth: usize,
    push_channel_depth: usize,
    /// Side-effects from the core's action apply.
    driver_rx: tokio::sync::mpsc::Receiver<DriverMsg>,
    /// The live engine actor and its request sender, present only between the
    /// one dial and teardown.
    engine: Option<EngineState>,
    /// Serialized engine replies. Held as a separate field (not inside
    /// [`EngineState`]) so the `select!` branches over the engine channels
    /// borrow disjoint fields.
    reply_rx: Option<tokio::sync::mpsc::Receiver<EngineReply>>,
    /// Live typed pushes.
    push_rx: Option<tokio::sync::mpsc::Receiver<Push>>,
    /// Local push-overflow (`Lagged`) counts.
    lag_rx: Option<tokio::sync::mpsc::Receiver<u64>>,
    /// Armed per-call deadline timers: id → fire tick. Bounded by the kernel's
    /// in-flight + queue limits (§K12).
    timers: std::collections::HashMap<TimerId, Tick>,
    /// The shared clock origin: the loop and the runtime derive `Tick` from the
    /// same `Instant`, so a timer armed by the core fires at the wall time the
    /// loop computes.
    origin: Instant,
    /// Set `true` once teardown completes, so a `stop()` future resolves only
    /// after the engine is down (AC "Stop … awaits teardown").
    torn_down: watch::Sender<bool>,
}

/// The live engine actor and its request sender.
struct EngineState {
    actor: DirectEngineActor,
    requests_tx: tokio::sync::mpsc::Sender<EngineRequest>,
}

impl DirectDriverTask {
    /// Build the loop state. The caller spawns [`run`](Self::run).
    pub(crate) fn new(
        runtime: Weak<DirectRuntime>,
        data_dir: PathBuf,
        request_channel_depth: usize,
        push_channel_depth: usize,
        driver_rx: tokio::sync::mpsc::Receiver<DriverMsg>,
        origin: Instant,
        torn_down: watch::Sender<bool>,
    ) -> Self {
        Self {
            runtime,
            data_dir,
            request_channel_depth,
            push_channel_depth,
            driver_rx,
            engine: None,
            reply_rx: None,
            push_rx: None,
            lag_rx: None,
            timers: std::collections::HashMap::new(),
            origin,
            torn_down,
        }
    }

    /// The single event loop. Selects over driver side-effects, engine replies,
    /// live pushes, local push-overflow, and the next due deadline timer.
    pub(crate) async fn run(mut self) {
        loop {
            let next_timer = self.next_timer_instant();
            tokio::select! {
                // A driver side-effect from the core's action apply. `None` means
                // every handle dropped: leave the loop and tear down.
                msg = self.driver_rx.recv() => match msg {
                    Some(msg) => self.on_driver_msg(msg).await,
                    None => break,
                },
                // A serialized engine reply.
                Some(reply) = recv_opt(self.reply_rx.as_mut()) => {
                    self.on_engine_reply(reply);
                },
                // A live push.
                Some(push) = recv_opt(self.push_rx.as_mut()) => {
                    let generation = self.generation();
                    self.feed(Input::Inbound(Inbound::Push { generation, push }));
                },
                // The push forward task lagged the engine broadcast: surface it
                // as local overflow so the reconciler resyncs.
                Some(dropped) = recv_opt(self.lag_rx.as_mut()) => {
                    self.on_lag(dropped);
                },
                // The next per-call deadline is due.
                () = sleep_until_opt(next_timer) => self.drain_due_timers(),
            }
        }
        self.shutdown().await;
    }

    /// Perform one side-effect the core asked for.
    async fn on_driver_msg(&mut self, msg: DriverMsg) {
        match msg {
            DriverMsg::Send(frame) => self.on_send(frame),
            DriverMsg::ArmTimer { id, at } => {
                self.timers.insert(id, at);
            }
            DriverMsg::CancelTimer(id) => {
                self.timers.remove(&id);
            }
            DriverMsg::Dial { token } => self.on_dial(token),
            DriverMsg::CancelDial => self.on_cancel_dial().await,
        }
    }

    /// Route, resolve, and dispatch one request to the engine. A parse/route/
    /// resolve failure settles a typed error **without touching the engine**
    /// (§5.3 steps 1–3); a full or closed request channel — unreachable while
    /// the channel is sized to the kernel's in-flight bound — settles honestly
    /// rather than blocking the loop (§9).
    fn on_send(&mut self, frame: WireFrame) {
        let Some(engine) = self.engine.as_ref() else {
            // No live engine to accept a send: the core only sends while Ready,
            // so this is unreachable, but fail honestly rather than strand.
            self.settle(frame.id, WireReply::Err(ApiError::NotReady));
            return;
        };
        let call = match bridge::resolve(frame.op, frame.input.as_str()) {
            Ok(call) => call,
            Err(err) => {
                self.settle(frame.id, WireReply::Err(err));
                return;
            }
        };
        let request = EngineRequest {
            id: frame.id,
            call,
            op_id: frame.op_id,
        };
        if let Err(err) = engine.requests_tx.try_send(request) {
            let id = match &err {
                tokio::sync::mpsc::error::TrySendError::Full(request)
                | tokio::sync::mpsc::error::TrySendError::Closed(request) => request.id,
            };
            self.settle(id, WireReply::Err(ApiError::NotReady));
        }
    }

    /// The one and only dial: acquire ownership and open the engine actor. On
    /// success the core moves to `Ready` (`Connected`, generation 1); on failure
    /// (dir already owned, or `Engine::new` error) it moves to terminal `Failed`
    /// (`GateRefused`, no retry).
    fn on_dial(&mut self, token: u64) {
        match DirectEngineActor::open(
            &self.data_dir,
            self.request_channel_depth,
            self.push_channel_depth,
        ) {
            Ok((actor, channels)) => {
                let requests_tx = actor.requests();
                self.engine = Some(EngineState { actor, requests_tx });
                self.reply_rx = Some(channels.reply_rx);
                self.push_rx = Some(channels.push_rx);
                self.lag_rx = Some(channels.lag_rx);
                // In-process, a daemon restart is a client restart, so the
                // incarnation is a fixed constant — the fence never fires, and
                // the driver never emits Interrupted, so no replay window ever
                // opens (§5.3).
                self.feed(Input::Connected {
                    token,
                    incarnation: jeliya_api::Incarnation::new("direct-in-process"),
                });
            }
            Err(_) => self.feed(Input::GateRefused { token }),
        }
    }

    /// Total stop: tear the live engine down and await it, then signal the
    /// stop future's completion. The kernel's `Input::Stop` already settled
    /// every accepted call before emitting `CancelDial`.
    async fn on_cancel_dial(&mut self) {
        self.teardown_engine().await;
        let _ = self.torn_down.send(true);
    }

    /// Feed a serialized engine reply, and — when the engine sequenced a
    /// `daemon.stop` — run the same teardown as an explicit stop after the reply
    /// is delivered.
    fn on_engine_reply(&mut self, reply: EngineReply) {
        self.settle(reply.id, reply.result);
        if reply.stop_after_reply {
            // Drive the same total-stop path an explicit `stop()` would: the
            // core settles anything else outstanding and emits `CancelDial`,
            // which the loop turns into teardown. Single teardown entry point.
            self.feed(Input::Stop);
        }
    }

    /// Surface local push-overflow as a `ClientEvent::Lagged` so the reconciler
    /// re-baselines (`ResyncReason::LocalOverflow`). Emitted straight onto the
    /// event bus (§5.3 route (b)): the wire push shape cannot represent a local
    /// signal, so this is the honest place to synthesize it.
    fn on_lag(&self, dropped: u64) {
        if let Some(runtime) = self.runtime.upgrade() {
            runtime.emit_lagged(dropped);
        }
    }

    /// Fire every timer whose deadline has elapsed, earliest first, re-checking
    /// after each (a settle may arm or cancel timers).
    fn drain_due_timers(&mut self) {
        loop {
            let now = self.now();
            let due = self
                .timers
                .iter()
                .filter(|(_, at)| **at <= now)
                .min_by_key(|(id, at)| (**at, id.0))
                .map(|(id, _)| *id);
            match due {
                Some(id) => {
                    self.timers.remove(&id);
                    self.feed(Input::TimerFired(id));
                }
                None => break,
            }
        }
    }

    /// Settle one call by feeding its reply to the core on the current
    /// generation.
    fn settle(&self, id: RequestId, result: WireReply) {
        let generation = self.generation();
        self.feed(Input::Inbound(Inbound::Reply {
            generation,
            id,
            result,
        }));
    }

    /// Feed one input to the kernel core, if the backend is still alive.
    fn feed(&self, input: Input) {
        if let Some(runtime) = self.runtime.upgrade() {
            runtime.drive(input);
        }
    }

    /// The current connection generation (1 while Ready; 0 before the first
    /// connect). Read from the core so stale traffic is fenced correctly.
    fn generation(&self) -> u64 {
        self.runtime
            .upgrade()
            .map_or(0, |runtime| runtime.generation())
    }

    /// Tear the engine down if one is live (idempotent). Drops the engine
    /// receive halves first: the dispatch task's reply sends then fail fast
    /// (rather than blocking the drain) while `teardown` awaits it, since the
    /// kernel's stop already settled every accepted call.
    async fn teardown_engine(&mut self) {
        self.reply_rx = None;
        self.push_rx = None;
        self.lag_rx = None;
        if let Some(engine) = self.engine.take() {
            let EngineState { actor, requests_tx } = engine;
            // Drop the loop's request-sender clone *before* awaiting teardown:
            // the dispatch task's `requests_rx` closes only once every sender is
            // gone, and `teardown` awaits that task, so a lingering clone here
            // would deadlock the drain.
            drop(requests_tx);
            let _ = actor.teardown().await;
        }
    }

    /// Final teardown when the loop exits (all handles dropped). Tears down any
    /// live engine and signals completion for any pending stop future.
    async fn shutdown(&mut self) {
        self.teardown_engine().await;
        let _ = self.torn_down.send(true);
    }

    /// The current logical time, from the shared clock origin (1 tick = 1 ms).
    fn now(&self) -> Tick {
        Tick(self.origin.elapsed().as_millis() as u64)
    }

    /// The wall instant the next due timer should fire at, or `None` when no
    /// timer is armed.
    fn next_timer_instant(&self) -> Option<Instant> {
        self.timers
            .values()
            .min()
            .map(|at| self.origin + Duration::from_millis(at.0))
    }
}

/// Await a receiver if present, else pend forever — so a `select!` branch over
/// an absent engine channel simply never fires.
async fn recv_opt<T>(rx: Option<&mut tokio::sync::mpsc::Receiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Sleep until `deadline` if present, else pend forever — so a `select!` timer
/// branch with no armed timer never fires.
async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}
