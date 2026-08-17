//! The native async WebSocket adapter (#172): `WsNative`.
//!
//! Binds the transport-independent kernel (#168) to a real `tokio` +
//! `tokio-tungstenite` transport that dials a **loopback** `jeliyad`, using the
//! reusable [`jeliya_supervisor::TargetResolver`] (#170) as the injected source
//! of *verified* loopback endpoint + token facts. It is the sole native
//! protocol-v2 WebSocket client the Dioxus desktop shell uses.
//!
//! ## What lands here
//!
//! 1. [`source`] — the injected [`TargetSource`] seam ([`Dial`] /
//!    [`DialResolveError`]) plus the provided
//!    `impl TargetSource for TargetResolver`, with the fail-closed-vs-retry
//!    classification of every [`jeliya_supervisor::SupervisorError`].
//! 2. [`runtime`] — [`connect_ws_native`], the native `DriverIo` (`NativeIo`)
//!    that hands the sans-IO core's effects to async tasks, and the
//!    connection/timer bookkeeping.
//! 3. [`ws_native`] — the dial → generation/hello agreement → serve → reconnect
//!    → stop state machine, framed through the protocol-v2 codec (#164).
//! 4. [`clock`] — the monotonic `Instant` → logical `Tick` mapping (1 ms =
//!    1 tick).
//!
//! ## Security posture (issue "Security and correctness")
//!
//! - The dial URL is `ws://127.0.0.1:<port>` built by the supervisor from a
//!   hardcoded loopback + a validated port; a portfile advertising a
//!   non-loopback endpoint is rejected before any dial. The adapter never
//!   parses a host out of untrusted portfile data.
//! - The bearer is a [`jeliya_supervisor::Redacted`] exposed **only** when the
//!   `Authorization` header is built — never a URL, a log line, a `Debug`, or a
//!   value the [`ClientHandle`](crate::ClientHandle) surfaces, so it is
//!   unreachable from WebView JS.
//! - "Connected" is never reported before three independent agreement checks:
//!   the resolver's health/validation proof, the daemon's upgrade gate, and a
//!   matching `hello` generation.

mod clock;
mod runtime;
mod source;
mod ws_native;

pub use runtime::{connect_ws_native, NativeClientConfig, NativeError};
pub use source::{Dial, DialResolveError, TargetSource};
