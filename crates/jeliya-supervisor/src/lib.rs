//! Reusable owned/adopted `jeliyad` supervisor for native control planes (#170).
//!
//! Any native control plane — the Dioxus desktop shell first, but also an agent
//! host or a diagnostic tool — uses this crate to obtain a live, correctly-owned
//! `jeliyad` and a freshly-validated connection target for every dial, while
//! enforcing the documented single-instance, token, portfile, parent-death, and
//! ownership invariants. The authoritative statement of the contract is the
//! daemon in `crates/jeliyad` and `docs/protocol-v2.md`; where this crate and
//! those disagree, they are right and this crate has a bug.
//!
//! The design deliberately **splits process ownership from target resolution**:
//!
//! - [`Supervisor`] spawns-or-adopts and yields a [`Sidecar`] — the non-`Clone`
//!   process-ownership handle. Only a [`Sidecar`] can stop a daemon, and only if
//!   it [`is owned`](Sidecar::is_owned).
//! - [`Sidecar::target_resolver`] hands a transport a cheap, cloneable
//!   [`TargetResolver`] that re-reads and re-validates the portfile on **every**
//!   call, producing a [`DialTarget`] (a loopback WS URL carrying the
//!   generation-gate query, plus a redacted bearer token). A transport can hold
//!   the resolver without being able to kill the process.
//!
//! What this crate is **not**: no UI (headless), no WebSocket request semantics
//! (that is the client kernel/codec), no token in any surface a WebView can read
//! (the token reaches only the native transport as `Authorization: Bearer`), and
//! no killing of an incumbent that cannot be **proven** to own the exact data
//! directory. Clean-slate: it recognizes exactly the built-against protocol and
//! storage generation and treats every other generation as incompatible, never
//! as something to migrate.
//!
//! Boundaries held by construction (asserted in `tests/boundaries.rs`): no
//! dependency on Dioxus, Iroh, `jeliya-core`, the `jeliyad` binary crate, or
//! `jeliya-client`, and the crate is never reachable from a `wasm32` build.

// `unsafe_code = "forbid"` is inherited from the workspace lints; the one place
// a raw signal is needed (`killpg`/`kill`) goes through `nix`'s safe wrappers.
#![deny(missing_docs)]

mod error;
mod generation;
mod health;
mod portfile;
mod process;
mod redact;
mod sidecar;
mod supervisor;
mod target;
mod validate;

pub use error::{BoxFuture, CallerRpcError, SupervisorError};
pub use generation::{Generation, SeenGeneration};
pub use portfile::Portfile;
pub use redact::Redacted;
pub use sidecar::{Sidecar, Teardown};
pub use supervisor::{Supervisor, SupervisorConfig, Timeouts};
pub use target::{DialTarget, TargetResolver};

// The single-definition discovery object the portfile's generation axes reuse
// (protocol-v2 Layer 0 — "defined once in jeliya-api"), re-exported so a
// consumer names one `Limits`.
pub use jeliya_api::Limits;
