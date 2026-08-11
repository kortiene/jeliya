//! The injectable `PlatformServices` boundary for the Jeliya clean-slate stack
//! (#174).
//!
//! This crate is the **single platform-authority contract** the shared Dioxus
//! UI uses, injected separately from [`jeliya_api`] view models and the
//! [`jeliya_client`](https://docs.rs)`::ClientHandle` seam. The authoritative
//! record of the decision is `docs/dioxus-architecture.md` §"Decision 4 — one
//! seam, four adapters, one platform boundary" (and the layering table in
//! §"Decision 3"); the product rules it enforces are
//! `docs/product-behavior-contract.md` §"PlatformServices" and §"Preferences
//! and device-local persistence". Where this crate and those records (or
//! `docs/protocol-v2.md`) disagree, the record is right and this crate has a
//! bug — say which in the PR, exactly as the architecture record requires of
//! every slice that tests against it.
//!
//! It delivers, and asserts by construction:
//!
//! - **Capability-oriented traits whose types distinguish platform object
//!   kinds.** A browser blob, a desktop filesystem path, and an Android
//!   `content://` URI are different Rust object kinds behind one opaque
//!   [`files::PickedSource`]; a shared component cannot read one as another
//!   because it can reach neither spelling (only a
//!   [`files::FileObjectKind`] discriminant).
//! - **A closed outcome taxonomy that never collapses.** [`CapabilityError`]
//!   keeps `Unavailable`, `Denied`, `Cancelled`, and typed failures apart, so a
//!   cancellation can never read as a success (the security requirement
//!   "cancellation must not become success").
//! - **Safe path/URL types and an allowlisted external-URL launcher.**
//!   [`launcher::SafeExternalUrl`] fails closed on any scheme outside a fixed
//!   allowlist, so an un-vetted URL cannot reach the platform launcher.
//! - **Explicit, honest storage ownership.** Preferences, secret custody, and
//!   protected-directory facts are three separate concerns, and durability is a
//!   queryable fact ([`storage::Durability`]) so the UI never pretends a
//!   browser reload restored state.
//! - **Representable lifecycle events.** App resume, process restoration, back,
//!   navigation, and window events are a closed [`lifecycle::LifecycleEvent`]
//!   model delivered on a loss-visible, bounded subscription.
//! - **A deterministic in-process fake for every service** (behind the `fake`
//!   feature), shaped as browser / desktop / Android fixtures and scriptable
//!   for denied / unavailable / cancelled outcomes.
//!
//! Boundaries this crate holds by construction (asserted in
//! `tests/boundaries.rs`, mirroring `jeliya-api`/`jeliya-client`/`jeliya-ui`):
//!
//! - **The library graph (default and `wasm32`) pulls no Iroh, `jeliya-core`,
//!   `jeliyad`, `jeliya-ffi`, WebSocket crate, native transport,
//!   `quinn`/`rustls`/`tokio`, `wry`/`tao`, `openssl-sys`, `native-tls`, or
//!   Dioxus.** The contract is renderer-agnostic; a Dioxus prop needs only
//!   `Clone + PartialEq`, which [`PlatformServices`] provides without depending
//!   on Dioxus.
//! - **No `serde_json::Value` in any public signature.** The crate consumes
//!   `jeliya_api` value types and never a raw JSON door.
//! - **No `tokio`, no wall clock, no `wasm-bindgen`-specific timing.**
//!   Concurrency and the lifecycle fan-out are executor-agnostic and
//!   `wasm32-unknown-unknown`-safe.
//!
//! This becomes the sole platform-authority contract for the new stack. Target
//! implementations (browser web-sys, desktop file dialogs, Android SAF/JNI) are
//! out of scope for this crate and land in M3–M5; the clean-slate cutover
//! applies (each target reads only new-format services and preferences; old
//! helpers and stored values are references only).

#![forbid(unsafe_code)]
#![deny(missing_docs)]
// The opaque-handle plumbing (the `SourceToken`/`ExportToken`/`BlobToken`
// constructors and accessors, and the private `token` fields) is consumed by
// the capability *implementations* — and the only implementations this crate
// ships are the deterministic fakes, behind the default-off `fake` feature.
// With no implementation compiled in, that crate-private plumbing is
// legitimately unused, so — and only then — allow dead code rather than scatter
// per-item attributes. When `fake` (or a future target impl) is compiled in,
// this is a no-op and dead-code linting is fully active. Mirrors
// `jeliya-client`'s `#![cfg_attr(not(feature = "mock"), allow(dead_code))]`.
#![cfg_attr(not(feature = "fake"), allow(dead_code))]

pub mod cancel;
pub mod clipboard;
pub mod error;
pub mod files;
pub mod launcher;
pub mod lifecycle;
pub mod navigation;
mod services;
pub mod storage;
pub mod window;

#[cfg(feature = "fake")]
pub mod fake;

pub use cancel::CancelToken;
pub use clipboard::{Clipboard, Share, ShareAnchor, ShareAttachment, ShareContent};
pub use error::{Availability, CapabilityError, FailureKind};
pub use files::{
    ExportTarget, ExportTargetKind, FileName, FileObjectKind, Files, LocalFileRef, Mime,
    PickedSource, ProgressSink, ShareableBlob, StageProgress,
};
pub use launcher::{SafeExternalUrl, UnsafeUrl, UrlLauncher};
pub use lifecycle::{
    BackgroundPhase, Lifecycle, LifecycleBus, LifecycleDelivery, LifecycleEvent,
    LifecycleSubscription,
};
pub use navigation::{Navigation, Route, RouteParseError};
pub use services::{Platform, PlatformServices};
pub use storage::{
    Durability, PreferenceKey, Preferences, PrivateDirectory, Secret, SecretKey, SecretStore,
    WriteOutcome,
};
pub use window::{WindowActions, WindowCommand, WindowEvent};

/// A boxed, `'a`-bounded future — the return shape every asynchronous
/// capability method uses so the erased implementations stay behind
/// `Arc<dyn …>` and the traits remain object-safe.
///
/// This is a `!Send` boxed future (the `LocalBoxFuture` shape), because the
/// browser target is single-threaded and a future built over a `web-sys`
/// handle is not `Send`; requiring `Send` here — as `futures::future::BoxFuture`
/// does — would foreclose the M3 web implementation. It is spelled in `std` so
/// the `!Send` bound is explicit at the definition and no `Send`-carrying alias
/// can be reached by mistake.
pub type BoxFuture<'a, T> = core::pin::Pin<Box<dyn core::future::Future<Output = T> + 'a>>;

// Depend on `jeliya_api` for the ids and view-model types this crate traffics
// in (`RoomId`, `FileId`, served limits). This crate re-exports none of them,
// to avoid a second spelling — a component names `jeliya_api::RoomId` directly.
