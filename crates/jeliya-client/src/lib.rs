//! Lifecycle-aware UI-facing client seam for Jeliya protocol v2 (#167).
//!
//! This crate is the **single UI-facing Rust client contract** that shared
//! Dioxus components use. The authoritative record of the decision is
//! `docs/dioxus-architecture.md` §"Decision 4 — one seam, four adapters, one
//! platform boundary"; where this crate and that record (or `docs/protocol-v2.md`)
//! disagree, the record is right and this crate has a bug.
//!
//! It delivers, and asserts by construction:
//!
//! - **Compile-time request/output pairing.** [`ClientHandle::call`] is the one
//!   paired entry point; it re-uses [`jeliya_api::Operation`] to bind each
//!   request to its `Output`, so a call cannot be made without knowing its
//!   reply type. There is no untyped "call by string".
//! - **An honest, observable lifecycle.** [`State`] is one enum every adapter
//!   maps its transport lifecycle into; [`ClientHandle::start`] /
//!   [`ClientHandle::stop`] and [`ClientEvent::StateChanged`] make transitions
//!   explicit without polling.
//! - **Multi-consumer events.** [`ClientHandle::subscribe`] returns an
//!   independent [`EventSubscription`] each time; every subscription observes
//!   every event, so no consumer can silently steal another's pushes. Replies
//!   never travel on the event stream.
//! - **An error model that preserves may-have-executed.** [`CallError`]
//!   separates wire failures from queue / timeout / cancel / gap / local
//!   failures, and [`CallError::execution`] is a total function classifying
//!   whether a failed mutation may have taken effect.
//! - **A deterministic mock** (behind the `mock` feature) that scripts
//!   responses, errors, push-before-response, gaps, cancellation, and shutdown
//!   with no wall clock, no timers, and no reliance on task-scheduling order.
//!
//! Boundaries this crate holds by construction (asserted in
//! `tests/boundaries.rs`, mirroring `jeliya-api`):
//!
//! - **No Iroh, WebSocket, native transport, `tao`/`wry`, or Dioxus in the
//!   library.** The seam is transport-independent and backend erasure stays
//!   internal; Dioxus appears only as an example/dev dependency for the
//!   verification component. `PlatformServices` (#174) is injected separately.
//! - **No `serde_json::Value` in any public signature.** The erased boundary
//!   carries a JSON *text* newtype, so the token appears nowhere in source.
//! - **No `tokio`, no wall clock, no scattered `cfg`.** Concurrency primitives
//!   are executor-agnostic and `wasm32-unknown-unknown`-safe; deadlines are the
//!   kernel/adapter's concern (#168), never the seam's.
//!
//! This becomes the sole UI client contract for the new stack; legacy clients
//! are not runtime fallbacks (clean-slate cutover).

#![forbid(unsafe_code)]
#![deny(missing_docs)]
// The seam's internal backend plumbing (the `ClientBackend` trait, the erased
// call boundary, and the event fan-out) is exercised by a backend. In #167 the
// only backend is the deterministic mock, behind the default-off `mock`
// feature; #168 adds the real adapters. With no backend feature enabled the
// plumbing is legitimately unused, so — and only then — allow dead code rather
// than scatter per-item attributes. When any backend is compiled in, this is a
// no-op and dead-code linting is fully active.
#![cfg_attr(not(feature = "mock"), allow(dead_code))]

mod backend;
mod error;
mod event;
mod handle;
mod kernel;
pub mod media;
mod reconcile;
mod stream;

// The #175 parameterized adapter contract suite core: the ten contracts, the
// `Rig` seam adapter rigs implement, and the shared evidence helpers.
// Default-off like the other test-scaffolding features (`mock`,
// `test-transport`) so the library's normal build carries none of it. CI
// runs the matrix with:
//   cargo test -p jeliya-client \
//     --features "contract mock ws-native direct" --test contract_suite
#[cfg(feature = "contract")]
pub mod contract;

// The browser WebSocket/session adapter (#171). Target-cfg + feature gated so
// the wasm-only browser crates never enter the native library tree (the
// `cargo tree` boundary test) and the module is invisible to the native build.
#[cfg(all(target_arch = "wasm32", feature = "ws-web"))]
mod ws_web;
// The adapter's media REGISTRY is pure Rust — no browser types — so it (and
// its unit tests: the bounded-registration eviction and the one-Produced-
// per-grant invariant) also compiles on the host under the feature, giving
// the logic a genuinely executed test target outside the browser. The wasm
// build consumes the same file through `ws_web::media`.
#[cfg(all(feature = "ws-web", not(target_arch = "wasm32")))]
#[path = "ws_web/media.rs"]
mod ws_web_media;
// The native async WebSocket adapter (#172): binds the sans-IO kernel to a real
// tokio + tokio-tungstenite transport dialing a loopback `jeliyad` via the
// reusable supervisor target resolver (#170). Default-off and native-only — the
// `pub(crate)` `Transport`/`Driver`/`DriverIo` seams live inside this crate, so
// the adapter lands here too, under `src/adapter/**` (outside the sans-IO
// kernel/reconcile scan). The web (wasm32) build never enables `ws-native`.
#[cfg(all(feature = "ws-native", not(target_arch = "wasm32")))]
mod adapter;
// The Android in-process DirectClient adapter (#173): the fourth kernel adapter,
// binding the bounded kernel core to the typed `jeliya-core` `Engine` in-process
// through one serialized actor. Native-only and behind the default-off `direct`
// feature, so it never enters the wasm build or the library's transport-free
// dependency tree (asserted by `tests/boundaries.rs`); its `tokio`/clock/engine
// machinery lives entirely here, never under `src/kernel/**` or
// `src/reconcile/**`.
#[cfg(all(not(target_arch = "wasm32"), feature = "direct"))]
mod direct;

#[cfg(feature = "mock")]
pub mod mock;

pub use error::{CallError, Execution, LocalError};
pub use event::{ClientEvent, EventSubscription, RoomPush, State};
pub use handle::{ClientHandle, Dedup};
pub use kernel::{KernelConfig, KernelLimits, StreamLimits, TickDelta};
pub use reconcile::{
    ReconcileConfig, ReconcileError, ReconcileLimits, Reconciler, ResyncReason, ResyncRequired,
    RoomUpdate, RoomUpdateSubscription, RoomView,
};
pub use stream::{StreamCall, StreamCancel};

// The browser adapter's public constructor and its injectable endpoint/session
// seam (#171). Only present on `wasm32-unknown-unknown` with `ws-web`; the
// native seam is untouched.
#[cfg(all(not(target_arch = "wasm32"), feature = "direct"))]
pub use direct::{connect_direct, DirectConfig, OwnershipError};
#[cfg(all(target_arch = "wasm32", feature = "ws-web"))]
pub use ws_web::{
    connect_ws_web, Endpoint, ExplicitResolver, GetTokenResolver, SessionError, SessionResolver,
    WsWebConfig,
};

// The deterministic in-memory kernel driver and its controller are the
// reference substrate the four real adapters (#171/#172/#173) are diffed
// against (#175). They ship behind `test-transport` (default-off) so the
// library's normal build carries no test scaffolding, mirroring how the mock
// backend ships behind `mock`.
#[cfg(feature = "test-transport")]
pub use kernel::{KernelController, SentFrame, SentRecord};

// The native adapter's public construction surface (#172). `connect_ws_native`
// builds a `ClientHandle` over the native WebSocket driver; `TargetSource` is
// the injected resolver seam (the supervisor's `TargetResolver` implements it),
// and `Dial`/`DialResolveError`/`NativeClientConfig`/`NativeError` are its
// supporting types. Native-only and behind the default-off `ws-native` feature.
#[cfg(all(feature = "ws-native", not(target_arch = "wasm32")))]
pub use adapter::{
    connect_ws_native, Dial, DialResolveError, NativeClientConfig, NativeError, TargetSource,
};

// The erasure is internal: `ClientBackend`, `ErasedCall`, and `RawJson` are
// deliberately never exported. Depend on `jeliya_api` for the typed operations,
// outputs, pushes, errors, ids, and shared value types — this crate re-exports
// none of them, to avoid a second spelling.
