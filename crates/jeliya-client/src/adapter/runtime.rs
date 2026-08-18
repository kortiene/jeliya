//! The reusable real async driver runtime and the native `DriverIo`.
//!
//! [`connect_ws_native`] builds a kernel-backed [`ClientHandle`] over
//! [`NativeIo`], the [`DriverIo`] that hands the sans-IO core's five
//! transport-touching effects to `tokio` tasks and a real socket. It reuses the
//! kernel's one tested delivery/wake shell ([`Runtime`]) rather than
//! duplicating the deferred-wake machinery; the async tasks feed inputs back to
//! the core through [`Runtime::inject`], the moral equivalent of the in-memory
//! controller's `drive_serialized` driven by real events.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::handle::ClientHandle;
use crate::kernel::core::Input;
use crate::kernel::driver_io::DriverIo;
use crate::kernel::timing::{Tick, TimerId};
use crate::kernel::transport::WireFrame;
use crate::kernel::{KernelBackend, Runtime};
use crate::KernelConfig;

use super::clock::Clock;
use super::source::TargetSource;
use super::ws_native;

/// Construction inputs for the native adapter. The per-call deadlines,
/// backoff, and stall/stream timers all flow through the core's *logical*
/// timers ([`KernelConfig`]); these two deadlines bound the dial and the
/// `hello` wait specifically, which have no logical timer of their own.
#[derive(Clone, Debug)]
pub struct NativeClientConfig {
    /// The kernel's bounds and jitter seed. **`stable_principal` is forced to
    /// `false`** by [`connect_ws_native`] regardless of this value: replay is
    /// unsafe for a socket adapter until #270's daemon-incarnation fence lands,
    /// so this slice never enables it (spec §7.8).
    pub kernel: KernelConfig,
    /// How long one dial (`connect_async`) may take before it is treated as a
    /// transient failure.
    pub connect_timeout: Duration,
    /// How long to wait for the daemon's first `hello` frame after a `101`
    /// upgrade before treating the attempt as a transient failure.
    pub hello_timeout: Duration,
}

impl Default for NativeClientConfig {
    fn default() -> Self {
        Self {
            kernel: KernelConfig::default(),
            connect_timeout: Duration::from_secs(10),
            hello_timeout: Duration::from_secs(5),
        }
    }
}

/// The public error surface of the native constructor/driver. Per-call and
/// lifecycle errors continue to flow through the seam's existing
/// `CallError`/`State`/`ClientEvent` model — this adds no new call-error
/// variants.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum NativeError {
    /// No `tokio` runtime is available to borrow. The adapter never constructs
    /// one; the caller must build the handle from within a runtime.
    Runtime,
    /// The configuration is invalid (for example a zero deadline).
    Config(String),
}

impl std::fmt::Display for NativeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NativeError::Runtime => {
                f.write_str("no tokio runtime to borrow; build the handle inside a runtime")
            }
            NativeError::Config(why) => write!(f, "invalid native client configuration: {why}"),
        }
    }
}

impl std::error::Error for NativeError {}

/// Bounded dial/hello deadlines, threaded to the dial task.
#[derive(Clone, Copy)]
pub(crate) struct Deadlines {
    pub(crate) connect: Duration,
    pub(crate) hello: Duration,
}

/// The live connection's I/O state, shared between [`NativeIo`] and the dial/
/// read/write tasks *without* the kernel's `Shared` lock, so a task can install
/// or clear it without deadlocking against a `send` in progress.
#[derive(Default)]
pub(crate) struct ConnRegistry {
    /// The generation the current live connection runs under (0 before any
    /// connect). A read/write task clears the writer only when this still names
    /// its own connection, so a delayed teardown cannot clobber a successor.
    pub(crate) generation: u64,
    /// The live connection's bounded write channel, or `None` when
    /// disconnected. `NativeIo::send` pushes encoded Text frames here.
    pub(crate) writer: Option<mpsc::Sender<Message>>,
    /// The connection's read task.
    pub(crate) read_task: Option<JoinHandle<()>>,
    /// The connection's write task.
    pub(crate) write_task: Option<JoinHandle<()>>,
    /// The connection's keepalive task (periodic Ping under the served
    /// `idle_timeout_ms`). `None` off the native adapter and when the served
    /// timeout is 0.
    pub(crate) keepalive_task: Option<JoinHandle<()>>,
}

impl ConnRegistry {
    /// Abort the connection's read/write/keepalive tasks and drop the writer.
    /// Aborting an already-finished task is harmless; dropping the writer
    /// closes the write channel so its task ends.
    pub(crate) fn tear_down(&mut self) {
        if let Some(task) = self.read_task.take() {
            task.abort();
        }
        if let Some(task) = self.write_task.take() {
            task.abort();
        }
        if let Some(task) = self.keepalive_task.take() {
            task.abort();
        }
        self.writer = None;
    }
}

/// The armed native timers, shared with each timer task so a *fired* timer can
/// remove its own handle without reaching back through the kernel's `Shared`
/// lock (the core emits no `CancelTimer` for a timer that fired, so the map
/// would otherwise accumulate completed handles — an unbounded growth §K12
/// forbids).
type Timers = Arc<Mutex<HashMap<TimerId, JoinHandle<()>>>>;

/// The native [`DriverIo`]: realizes the core's effects against `tokio` tasks,
/// a monotonic clock, and a real socket. Held behind the kernel's `Shared`
/// lock, so every method is non-blocking (channel pushes and task spawns only).
pub(crate) struct NativeIo {
    /// Back-edge to the shell, so spawned tasks can `inject` inputs.
    runtime: Weak<Runtime>,
    /// The borrowed runtime handle (the adapter never builds a runtime); used
    /// to spawn from the synchronous `Shared`-lock context.
    handle: Handle,
    /// The injected resolver — runs on **every** dial attempt.
    source: Arc<dyn TargetSource>,
    /// The live connection's I/O state.
    conn: Arc<Mutex<ConnRegistry>>,
    /// The armed timers.
    timers: Timers,
    /// The in-progress dial task, if any.
    dial: Option<JoinHandle<()>>,
    /// Dial/hello deadlines.
    deadlines: Deadlines,
    /// The bounded write-channel depth (≥ in-flight cap, so a reconnect's flush
    /// never wedges on backpressure).
    write_buffer: usize,
    /// The monotonic logical clock.
    clock: Clock,
    /// A synchronous write failure observed during the current apply batch —
    /// surfaced to the core as `Interrupted` after the batch (a full or closed
    /// write channel is a connection loss the core must reclassify against).
    send_failed: bool,
}

impl NativeIo {
    #[allow(clippy::too_many_arguments)]
    fn new(
        runtime: Weak<Runtime>,
        handle: Handle,
        source: Arc<dyn TargetSource>,
        conn: Arc<Mutex<ConnRegistry>>,
        deadlines: Deadlines,
        write_buffer: usize,
    ) -> Self {
        Self {
            runtime,
            handle,
            source,
            conn,
            timers: Arc::new(Mutex::new(HashMap::new())),
            dial: None,
            deadlines,
            write_buffer,
            clock: Clock::new(),
            send_failed: false,
        }
    }
}

impl DriverIo for NativeIo {
    fn now(&self) -> Tick {
        self.clock.now()
    }

    // Byte-stream media is not wired on the native adapter yet: a stream
    // record cannot be framed onto the wire from here, so these effects are
    // dropped and the stream's stall timer settles the call honestly
    // (Timeout) rather than reporting progress that never happened. The
    // debug assertion keeps the gap loud in tests until the native media
    // drive lands (the codec-side framing exists: jeliya-codec).
    fn send_record(&mut self, intent: crate::kernel::transport::StreamRecordIntent) {
        let _ = intent;
        debug_assert!(
            false,
            "stream records are not yet wired on the native adapter"
        );
    }

    fn produce(
        &mut self,
        id: jeliya_api::RequestId,
        call_id: crate::kernel::inflight::CallId,
        up_to: u64,
    ) {
        let _ = (id, call_id, up_to);
        debug_assert!(false, "stream media is not yet wired on the native adapter");
    }

    fn write_sink(
        &mut self,
        id: jeliya_api::RequestId,
        call_id: crate::kernel::inflight::CallId,
        offset: u64,
        len: u64,
    ) {
        let _ = (id, call_id, offset, len);
        debug_assert!(false, "stream media is not yet wired on the native adapter");
    }

    fn send(&mut self, frame: WireFrame) {
        // The codec owns all JSON: assemble the request envelope there, then
        // push one Text message to the live write channel. The payload/op_id
        // are never logged (WireFrame Debug already redacts them).
        let bytes = jeliya_codec::encode_request(
            frame.id.get(),
            frame.op,
            frame.op_id.as_ref(),
            frame.input.as_str(),
        );
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            // Unreachable: encode_request always yields valid UTF-8 JSON.
            Err(_) => {
                self.send_failed = true;
                return;
            }
        };
        let message = Message::text(text);
        let registry = self.conn.lock().expect("conn registry poisoned");
        match registry.writer.as_ref() {
            // A full or closed channel is a connection loss: surface it as
            // Interrupted after this batch (a real driver cannot report a write
            // failure synchronously into the core without re-entering the lock).
            Some(writer) if writer.try_send(message).is_ok() => {}
            _ => {
                drop(registry);
                self.send_failed = true;
            }
        }
    }

    fn arm_timer(&mut self, id: TimerId, at: Tick) {
        let delay_ms = at.saturating_ms_from(self.clock.now());
        let runtime = self.runtime.clone();
        let timers = self.timers.clone();
        let task = self.handle.spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            // Remove our own handle before firing: a fired timer gets no
            // CancelTimer, so it must not linger in the map (dropping the
            // handle detaches, it does not abort this running task).
            timers.lock().expect("timers poisoned").remove(&id);
            if let Some(runtime) = runtime.upgrade() {
                runtime.inject(Input::TimerFired(id));
            }
        });
        if let Some(old) = self
            .timers
            .lock()
            .expect("timers poisoned")
            .insert(id, task)
        {
            old.abort();
        }
    }

    fn cancel_timer(&mut self, id: TimerId) {
        if let Some(task) = self.timers.lock().expect("timers poisoned").remove(&id) {
            task.abort();
        }
    }

    fn dial(&mut self, token: u64) {
        // The core issues one dial at a time; abort any predecessor.
        if let Some(old) = self.dial.take() {
            old.abort();
        }
        let runtime = self.runtime.clone();
        let source = self.source.clone();
        let conn = self.conn.clone();
        let deadlines = self.deadlines;
        let write_buffer = self.write_buffer;
        self.dial = Some(self.handle.spawn(async move {
            ws_native::run_dial(token, runtime, source, conn, deadlines, write_buffer).await;
        }));
    }

    fn cancel_dial(&mut self) {
        // Total stop / terminal failure: abort the dial and tear the live
        // connection down. Timers are cancelled individually by the core's
        // CancelTimer actions; `Drop` sweeps anything left.
        if let Some(task) = self.dial.take() {
            task.abort();
        }
        self.conn
            .lock()
            .expect("conn registry poisoned")
            .tear_down();
    }

    fn take_send_failed(&mut self) -> bool {
        std::mem::take(&mut self.send_failed)
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl Drop for NativeIo {
    fn drop(&mut self) {
        // Belt-and-braces teardown: even if the handle is dropped without a
        // stop, no task or socket survives (spec §7.7).
        if let Some(task) = self.dial.take() {
            task.abort();
        }
        for (_, task) in self.timers.lock().expect("timers poisoned").drain() {
            task.abort();
        }
        self.conn
            .lock()
            .expect("conn registry poisoned")
            .tear_down();
    }
}

/// Build a native-WebSocket-backed [`ClientHandle`].
///
/// `source` is the injected [`TargetSource`] (the supervisor's
/// [`TargetResolver`](jeliya_supervisor::TargetResolver) implements it). The
/// handle borrows the **current** `tokio` runtime — call this from within one,
/// or it returns [`NativeError::Runtime`]. The client does not dial until
/// [`ClientHandle::start`] is invoked.
pub fn connect_ws_native<S: TargetSource>(
    source: S,
    config: NativeClientConfig,
) -> Result<ClientHandle, NativeError> {
    let handle = Handle::try_current().map_err(|_| NativeError::Runtime)?;
    if config.connect_timeout.is_zero() || config.hello_timeout.is_zero() {
        return Err(NativeError::Config(
            "connect_timeout and hello_timeout must be non-zero".to_owned(),
        ));
    }

    // Enforce the safe replay default for a socket adapter: the dedup ledger is
    // in-memory and a daemon restart empties it, so replaying a mutation across
    // a reconnect could double-execute against a new incarnation. #172 never
    // opts in; #270's `hello` incarnation fence is the follow-up that will.
    let mut kernel = config.kernel;
    kernel.stable_principal = false;

    let source: Arc<dyn TargetSource> = Arc::new(source);
    let conn = Arc::new(Mutex::new(ConnRegistry::default()));
    let deadlines = Deadlines {
        connect: config.connect_timeout,
        hello: config.hello_timeout,
    };
    // A reconnect flushes up to the whole in-flight window at once; size the
    // write channel above it so that flush never trips synchronous backpressure.
    let write_buffer = (kernel.limits.in_flight as usize).saturating_add(64);

    let runtime = Runtime::new_cyclic_with_io(kernel, |weak| {
        Box::new(NativeIo::new(
            weak.clone(),
            handle.clone(),
            source.clone(),
            conn.clone(),
            deadlines,
            write_buffer,
        ))
    });

    Ok(ClientHandle::from_backend(Arc::new(KernelBackend::new(
        runtime,
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_disables_replay_and_bounds_deadlines() {
        let config = NativeClientConfig::default();
        assert!(!config.kernel.stable_principal);
        assert!(!config.connect_timeout.is_zero());
        assert!(!config.hello_timeout.is_zero());
    }

    #[test]
    fn construction_outside_a_runtime_is_a_runtime_error() {
        struct NeverResolves;
        impl TargetSource for NeverResolves {
            fn resolve(
                &self,
            ) -> futures::future::BoxFuture<
                'static,
                Result<super::super::source::Dial, super::super::source::DialResolveError>,
            > {
                Box::pin(async {
                    Err(super::super::source::DialResolveError::Transient(
                        "x".into(),
                    ))
                })
            }
        }
        // No current runtime here (a plain test thread), so construction fails
        // closed rather than panicking on the first spawn.
        let result = connect_ws_native(NeverResolves, NativeClientConfig::default());
        assert!(matches!(result, Err(NativeError::Runtime)));
    }

    #[test]
    fn zero_connect_timeout_rejects_with_config_error() {
        struct Stub;
        impl TargetSource for Stub {
            fn resolve(
                &self,
            ) -> futures::future::BoxFuture<
                'static,
                Result<super::super::source::Dial, super::super::source::DialResolveError>,
            > {
                Box::pin(futures::future::pending())
            }
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build runtime");
        let _guard = rt.enter();
        let config = NativeClientConfig {
            connect_timeout: std::time::Duration::ZERO,
            ..Default::default()
        };
        let result = connect_ws_native(Stub, config);
        assert!(
            matches!(result, Err(NativeError::Config(_))),
            "a zero connect_timeout must be rejected at construction"
        );
    }

    #[test]
    fn zero_hello_timeout_rejects_with_config_error() {
        struct Stub;
        impl TargetSource for Stub {
            fn resolve(
                &self,
            ) -> futures::future::BoxFuture<
                'static,
                Result<super::super::source::Dial, super::super::source::DialResolveError>,
            > {
                Box::pin(futures::future::pending())
            }
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build runtime");
        let _guard = rt.enter();
        let config = NativeClientConfig {
            hello_timeout: std::time::Duration::ZERO,
            ..Default::default()
        };
        let result = connect_ws_native(Stub, config);
        assert!(
            matches!(result, Err(NativeError::Config(_))),
            "a zero hello_timeout must be rejected at construction"
        );
    }
}
