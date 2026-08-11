//! Shared Dioxus UI crate for the Jeliya clean-slate stack (#176).
//!
//! This is the **production-shaped, system-WebView-targeted** component
//! library and application root. The authoritative record of the decision is
//! `docs/dioxus-architecture.md` — Decision 1 (one renderer, system WebView),
//! Decision 3 (layering and the allowed dependency direction), and Decision 5
//! (one embedded artifact). Where this crate and that record (or
//! `docs/protocol-v2.md` / `docs/dioxus-web-build.md`) disagree, the record is
//! right and this crate has a bug — say which in the PR, exactly as the
//! architecture record requires of every slice that tests against it.
//!
//! It composes the application from three **separately injected** inputs and
//! selects composition per target only at the crate root:
//!
//! - [`jeliya_api`] typed operations, outputs, pushes, errors, and **view
//!   models** — never a second spelling of any wire type, never
//!   `serde_json::Value`.
//! - [`jeliya_client::ClientHandle`] — the cloneable, lifecycle-aware client
//!   seam. #176 renders against the crate's deterministic **mock** (the
//!   reference behaviour); the real browser transport (`WsWeb`) is #168 and
//!   slots in behind the same handle later. **This crate does not open a
//!   socket.**
//! - [`PlatformServices`] — the injectable platform-authority boundary
//!   (persistence, URLs, clipboard, navigation). **Provisional pending #174**:
//!   this crate carries a minimal local seam with deterministic in-process
//!   implementations; when #174 lands, `jeliya-ui` adopts the canonical trait
//!   by replacing the local seam with a re-export — a mechanical change.
//!
//! Boundaries this crate holds by construction (asserted in
//! `tests/boundaries.rs`, mirroring `jeliya-api`/`jeliya-client`):
//!
//! - **The browser (`wasm32`) graph excludes Iroh and every native crate.**
//!   `jeliya-ui` depends on `jeliya-api`, `jeliya-client`, and (optionally,
//!   feature-gated) Dioxus. It must **not** depend on Iroh, `jeliya-core`,
//!   `jeliyad`, `jeliya-ffi`, a WebSocket crate, a native transport,
//!   `quinn`/`rustls`/`tokio`, `wry`/`tao`, or `openssl-sys`.
//! - **The renderer is optional and feature-gated.** The default build pulls
//!   no Dioxus, so the workspace MSRV job compiles this crate renderer-free
//!   and OpenSSL-free. The shared surface lives behind the `ui` feature; the
//!   browser entry point (`bin/web.rs`) behind `web`.
//! - **`jeliya-ui` reaches platform authority only through injected
//!   [`PlatformServices`]**, never directly, and `ClientHandle` and
//!   `PlatformServices` are injected **separately** (#174) — never entangled.
//! - **No platform business-logic `cfg` forks in shared components.** Target
//!   differences live only in [`compose`] and the per-target `bin`, and in the
//!   injected services — never scattered through [`components`].

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// The shared, renderer-agnostic surface. It compiles only under the `ui`
// feature (which pulls the optional Dioxus dependency with `minimal`), exactly
// like `jeliya-client`'s verification component lives behind `example`. With
// no renderer feature the crate is (essentially) empty, which is what keeps the
// MSRV `--workspace --all-targets` job renderer-free and OpenSSL-free.
#[cfg(feature = "ui")]
mod app;
#[cfg(feature = "ui")]
pub mod components;
#[cfg(feature = "ui")]
pub mod compose;
#[cfg(feature = "ui")]
mod services;
#[cfg(feature = "ui")]
mod state;

#[cfg(feature = "ui")]
pub use app::{AppRoot, AppRootProps};
#[cfg(feature = "ui")]
pub use services::{PlatformServices, PlatformServicesImpl, WebPlatformServices};
#[cfg(feature = "ui")]
pub use state::UiState;

// Depend on `jeliya_api` for the typed operations, outputs, pushes, errors,
// ids, and shared value types, and on `jeliya_client` for the seam — this
// crate re-exports neither, to avoid a second spelling.
