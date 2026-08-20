//! The #175 parameterized adapter contract suite core.
//!
//! One set of view-level contracts, written ONCE against [`ClientHandle`] and
//! the [`State`]/[`ClientEvent`] model, replayed against every adapter rig
//! (mock, WsNative against a real spawned `jeliyad`, DirectClient against a
//! real typed `Engine`). A contract that only one adapter satisfies by
//! accident becomes visible here as a matrix gap; a regression in any
//! adapter's view-level behavior fails the same test on that rig.
//!
//! What this module deliberately is NOT:
//!
//! - It is not a second protocol conformance corpus. Wire framing is the
//!   v2 corpus's job (`conformance/v2`); this suite pins the CLIENT-SIDE
//!   contract every shell component consumes: lifecycle states, typed call
//!   pairing, push-vs-reply separation, local bounds, cancellation
//!   classification, gate-refusal finality, ambiguous execution, transport
//!   loss, restart durability, and shutdown.
//! - It is not transport-aware. Contracts address the rig only through the
//!   [`Rig`] trait, so the same code compiles for a future wasm/web rig
//!   (175b): every wait is poll-counted (no `std::time::Instant` — unavailable
//!   on wasm32) and every sleep is rig-injected.
//!
//! Applicability is explicit, never silent: a contract that a rig cannot
//! honestly exercise carries a recorded [`Reason`] string in the test
//! matrix (`tests/contract_suite`), and a meta-test fails the suite if any
//! contract runs on no rig at all or any gap carries an empty reason.

use std::future::Future;
use std::pin::Pin;

use futures::{FutureExt, StreamExt};
use jeliya_api::RoomId;

use crate::event::ClientEvent;
use crate::handle::ClientHandle;
use crate::State;

pub mod contracts;

/// A contract failure: the human-readable evidence line. The suite fails
/// loud — a contract never "passes with concerns".
#[derive(Debug, Clone)]
pub struct ContractFailure(pub String);

impl std::fmt::Display for ContractFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ContractFailure {}

/// Every contract returns this.
pub type ContractResult = Result<(), ContractFailure>;

/// Fail a contract with a formatted evidence line.
macro_rules! fail {
    ($($arg:tt)*) => {
        return Err($crate::contract::ContractFailure(format!($($arg)*)))
    };
}
pub(crate) use fail;

/// What a scripted rig should make of the next `room.list`. Real-backend
/// rigs ignore this — their parking comes from the REAL kernel bounds in
/// [`RigConfig`]; the mock has no kernel, so the contract selects the
/// scripted shape of its pending call explicitly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PendingPlan {
    /// `room.list` resolves `ok` then a wire `op_id_conflict` (two queued
    /// entries: the happy contract takes the first, the retry contract
    /// takes both).
    #[default]
    Standard,
    /// `room.list` resolves with a LOCAL `QueueFull` (classification check).
    LocalQueueFull,
    /// `room.list` hangs as an UNSENT call.
    HangUnsent,
    /// `room.list` hangs as a SENT call.
    HangSent,
}

/// One rig's configuration. Rigs ignore the fields that do not apply to
/// them; contracts set the fields they need (bounds, budgets, daemon path)
/// and let defaults carry the rest.
#[derive(Clone, Debug)]
pub struct RigConfig {
    /// Test label used in failure lines.
    pub label: String,
    /// Scripted shape of the next pending call (mock rig only).
    pub plan: PendingPlan,
    /// Protocol version the native rig declares at the gate (C6 overrides
    /// it to a refused value).
    pub gate_version: u64,
    /// Kernel bound: max admitted-but-unsent calls before
    /// `QueueFull { resource: "calls" }`. `0` parks every call.
    pub queue_depth: u32,
    /// Kernel bound: max sent-and-awaiting calls. `0` never sends — a
    /// deterministic way to park a call unsent on the REAL kernel path.
    pub in_flight: u32,
    /// Reconnect attempt ceiling before `Failed`.
    pub max_reconnect_attempts: u32,
    /// Reconnect backoff base (mapped 1 tick = 1 ms, as the native clock
    /// defines).
    pub backoff_base_ms: u64,
    /// Reconnect backoff cap — the ceiling of the exponential window.
    pub backoff_cap_ms: u64,
    /// Dial deadline (native rig).
    pub connect_timeout_ms: u64,
    /// First-hello deadline after the 101 upgrade (native rig).
    pub hello_timeout_ms: u64,
    /// Persistent data directory (native daemon / direct engine).
    pub data_dir: std::path::PathBuf,
    /// `jeliyad` binary path (native rig; the rig FAILS if it is missing —
    /// a missing oracle must never read as a skipped contract).
    pub jeliyad_bin: std::path::PathBuf,
    /// Poll iterations a `wait_*` helper may spend before failing.
    pub wait_polls: usize,
    /// Milliseconds slept between polls (rig-injected).
    pub poll_ms: u64,
}

impl RigConfig {
    /// A config with the bounds a contract overrides per test. Deadlines are
    /// generous for CI; polls are sized for a local daemon (~instant hello,
    /// spawn ≤ 20 s).
    pub fn new(label: &str, data_dir: std::path::PathBuf, jeliyad_bin: std::path::PathBuf) -> Self {
        Self {
            label: label.to_owned(),
            plan: PendingPlan::Standard,
            gate_version: 2,
            queue_depth: 16,
            in_flight: 8,
            max_reconnect_attempts: 6,
            backoff_base_ms: 100,
            backoff_cap_ms: 400,
            connect_timeout_ms: 5_000,
            hello_timeout_ms: 5_000,
            data_dir,
            jeliyad_bin,
            wait_polls: 400,
            poll_ms: 50,
        }
    }

    /// Park every call unsent on the real kernel path (never sends, so the
    /// first call deterministically occupies the queue and stop/loss paths
    /// settle it as unsent).
    pub fn nothing_ever_sends(mut self) -> Self {
        self.in_flight = 0;
        self.queue_depth = 1;
        self
    }

    /// Fail fast after a loss: one reconnect attempt, short backoff.
    pub fn fail_fast_reconnect(mut self) -> Self {
        self.max_reconnect_attempts = 1;
        self.backoff_base_ms = 50;
        self.backoff_cap_ms = 100;
        self
    }

    /// A reconnect budget that comfortably outlasts a backend restart
    /// (~1–2 s of respawn): several attempts over a widening window, so the
    /// first losses against the dead endpoint cannot exhaust the budget
    /// before the new portfile exists.
    pub fn patient_reconnect(mut self) -> Self {
        self.max_reconnect_attempts = 12;
        self.backoff_base_ms = 250;
        self.backoff_cap_ms = 1_000;
        self
    }
}

/// One adapter rig: everything a contract needs to drive a backend it did
/// not script itself.
///
/// `wait_state`/`expect_state`/`drain_events` are provided in terms of
/// `handle`, `sleep_ms`, `config` — rigs implement only spawn/sleep plus
/// their backend-specific loss and restart verbs.
pub trait Rig: Sized {
    /// Human name for failure lines ("mock", "native", "direct").
    fn name(&self) -> &'static str;

    /// The handle contracts drive.
    fn handle(&self) -> &ClientHandle;

    /// This rig's config (budgets and bounds).
    fn config(&self) -> &RigConfig;

    /// Build the backend and a handle in `Idle`. Rigs that need an oracle
    /// (a daemon, an engine home) create it here and FAIL (not skip) when
    /// the oracle is missing.
    fn spawn(config: RigConfig) -> impl Future<Output = Result<Self, ContractFailure>>;

    /// Sleep `ms` milliseconds of the rig's clock. Native/direct sleep real
    /// time; a scripted rig yields instead (its waits are poll-counted).
    fn sleep_ms(&self, ms: u64) -> impl Future<Output = ()>;

    /// Take the handle to `Connecting` (dial/activate). The default forwards
    /// to `ClientHandle::start`; a rig whose backend needs more overrides.
    fn start(&mut self) {
        self.handle().start();
    }

    /// Identity provisioning after Ready. The daemon provisions a local
    /// subject at startup, so socket rigs need nothing here; a fresh
    /// in-process engine answers mutations with `subject_absent` until
    /// `subject.ensure` runs — the direct rig does it here. Default: no-op.
    fn on_ready(&mut self) -> impl Future<Output = ContractResult> {
        async { Ok(()) }
    }

    /// The contracts' shared prologue: start, reach Ready, run the rig's
    /// identity step. Every contract begins with this.
    fn bring_up(&mut self) -> impl Future<Output = Result<State, ContractFailure>> {
        async move {
            self.start();
            let state = self.wait_state(|s| s == State::Ready).await?;
            self.on_ready().await?;
            Ok(state)
        }
    }

    /// Whether this rig's adapter implements `stream.subscribe`. The direct
    /// adapter's actor routes stream.* ops to the engine dispatcher, which
    /// answers `subscription_unknown` — the stream.* actor deferral recorded
    /// in the #302 ledger — so C3 skips the wire subscription there and
    /// relies on the engine's own push forwarding. Default: supported.
    fn supports_stream_subscribe(&self) -> bool {
        true
    }

    /// Whether this rig's kernel ENFORCES admission bounds the contract can
    /// probe. Real-kernel rigs (native, direct) let C5 witness "parked
    /// unsent" through the queue: a probe call is refused with a local
    /// QueueFull while the parked call holds the single slot. The scripted
    /// mock has no admission machinery — its unsent premise is witnessed by
    /// the script itself (`Program::hang(false)`).
    fn witnesses_unsent_by_admission(&self) -> bool {
        true
    }

    /// Whether this rig's oracle ECHOES request inputs in its replies. A
    /// scripted oracle answers from a fixed script and cannot echo an input
    /// it never saw; contracts that pin echo behavior (the name round-trip
    /// in C2) assert it only where the oracle is real. Default: real.
    fn echoes_inputs(&self) -> bool {
        true
    }

    /// Poll `handle().state()` until `pred` holds or the config's poll
    /// budget elapses. Poll-counted (no `Instant`): portable to wasm32.
    fn wait_state(
        &mut self,
        pred: impl Fn(State) -> bool,
    ) -> impl Future<Output = Result<State, ContractFailure>> {
        async move {
            let (polls, poll_ms) = (self.config().wait_polls, self.config().poll_ms);
            for i in 0..polls {
                let state = self.handle().state();
                if pred(state) {
                    return Ok(state);
                }
                if i + 1 == polls {
                    break;
                }
                self.sleep_ms(poll_ms).await;
            }
            fail!(
                "[{}] state never satisfied the predicate; now {:?} after {polls} polls",
                self.name(),
                self.handle().state()
            )
        }
    }

    /// Assert the state NEVER satisfies `pred` while polling for
    /// `polls` rounds — the negative form used by the version-mismatch
    /// contract ("a gate-refused client is never Ready").
    fn expect_state_never(
        &mut self,
        pred: impl Fn(State) -> bool,
        polls: usize,
    ) -> impl Future<Output = ContractResult> {
        async move {
            let poll_ms = self.config().poll_ms;
            for _ in 0..polls {
                let state = self.handle().state();
                if pred(state) {
                    fail!(
                        "[{}] state reached {:?}, which the contract forbids",
                        self.name(),
                        state
                    );
                }
                self.sleep_ms(poll_ms).await;
            }
            Ok(())
        }
    }

    /// Pump a subscription for `polls` rounds, returning every event seen.
    /// Polling (rather than awaiting the stream) keeps this portable across
    /// rigs whose pumps live on another executor.
    fn drain_events(
        &mut self,
        sub: &mut crate::event::EventSubscription,
        polls: usize,
    ) -> impl Future<Output = Vec<ClientEvent>> {
        async move {
            let poll_ms = self.config().poll_ms;
            let mut out = Vec::new();
            for _ in 0..polls {
                while let Some(event) = sub.next().now_or_never().flatten() {
                    out.push(event);
                }
                self.sleep_ms(poll_ms).await;
            }
            while let Some(event) = sub.next().now_or_never().flatten() {
                out.push(event);
            }
            out
        }
    }

    /// Destroy the backend's transport without a close frame (native: kill
    /// the daemon process; scripted: the mock's `drop_connection`). Rigs
    /// without a transport fail the contract — they must be marked N/A in
    /// the matrix instead.
    fn sever_transport(&mut self) -> impl Future<Output = ContractResult> {
        std::future::ready(Err(ContractFailure(format!(
            "[{}] this rig has no transport to sever; mark the contract N/A with a reason",
            self.name()
        ))))
    }

    /// Restart the backend from the same persistent state (native: respawn
    /// the daemon on a fresh port from the same data dir; direct: rebuild
    /// the engine over the same home; scripted rigs have nothing to restart
    /// and must be marked N/A).
    fn restart_backend(&mut self) -> impl Future<Output = ContractResult> {
        std::future::ready(Err(ContractFailure(format!(
            "[{}] this rig cannot restart its backend; mark the contract N/A with a reason",
            self.name()
        ))))
    }
}

/// Evidence helpers shared by the contracts.
/// The first `StateChanged` entering `to`.
pub fn transitions_to(events: &[ClientEvent], to: State) -> bool {
    events.iter().any(|event| match event {
        ClientEvent::StateChanged { to: t, .. } => *t == to,
        _ => false,
    })
}

/// Every room-scoped push in the log for `room`.
pub fn room_pushes<'e>(
    events: &'e [ClientEvent],
    room: &RoomId,
) -> Vec<&'e crate::event::RoomPush> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Push(push) => match push {
                crate::event::RoomPush::Event { room_id, .. }
                | crate::event::RoomPush::Peer { room_id, .. } => (room_id == room).then_some(push),
                crate::event::RoomPush::Transfer { .. } => None,
            },
            _ => None,
        })
        .collect()
}

/// Box a contract future (used by the rig-side dispatch tables).
pub fn boxed<F: Future<Output = ContractResult> + 'static>(
    fut: F,
) -> Pin<Box<dyn Future<Output = ContractResult>>> {
    Box::pin(fut)
}
