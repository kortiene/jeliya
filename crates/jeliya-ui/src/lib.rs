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
//!   (files, persistence, lifecycle, URLs, clipboard/share, navigation, window
//!   actions). This is now the **canonical** contract from
//!   [`jeliya_platform`] (#174); `jeliya-ui` re-exports it and injects a
//!   deterministic fake shape. The former provisional local seam has been
//!   deleted.
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
// The canonical EN/FR localization layer (#177): the one authoritative catalog
// for the surviving stack, the formatting seam, wire/error display, and the
// Rust-facing identity-palette token source. Compiler-enforced key/placeholder
// parity between `l10n::En` and `l10n::Fr`; the node gates add the checks types
// cannot see (empty value, `fr==en`, plurals, French typography, literal scan).
#[cfg(feature = "ui")]
pub mod l10n;
// Host-testable Files/Pipes orchestration (#181 §5.B/§5.C): the pure decisions
// (truthful availability, served size limit, safe-preview policy, typed error
// mapping, cancellation) that drive the `ClientHandle` file/pipe operations and
// the `PlatformServices` file capabilities. Renderer- and `web-sys`-free and
// target-agnostic, so desktop (#184) / Android (#192) reuse them; the browser
// bindings live in `jeliya-platform-web`.
#[cfg(feature = "ui")]
pub mod files;
#[cfg(feature = "ui")]
pub mod pipes;
// The fresh, namespaced, versioned browser preference schema (#178 §6): the
// pure "decision" half of `WebPreferences` — namespace, envelope, key
// derivation, corrupt/unsupported-version recovery, and the enumerated
// legacy-key purge policy (data only; never a reader). Host-testable; the
// browser binding (`jeliya-platform-web`) supplies only the storage.
#[cfg(feature = "ui")]
pub mod prefs;
// The pure, renderer-agnostic core of the room Activity destination (#179): the
// exhaustive event projection, agent-status run folding + activity filters, the
// evidence-backed send state machine, the pure scroll math, and the fold of the
// reconciler's `RoomUpdate` stream into a per-room view. Host-testable; the thin
// Dioxus components in `components::{activity,composer,timeline_row}` render it
// and own DOM measurement through the mounted element API (Decision-3/-6).
#[cfg(feature = "ui")]
pub mod room;
// The global-shell logic (#178 §5/§8): responsive shell selection, the router
// (canonicalization + fail-safe + last-room restore), and the bootstrap/
// onboarding state machine. The pure decisions live here, host-testable; the
// `web-sys` bindings live in `jeliya-platform-web` and `compose`.
#[cfg(feature = "ui")]
pub mod shell;
// The browser `Platform` bindings (#178 §5.A): `web-sys` shims over the History
// API, browser lifecycle events, and the session-scoped preference/secret store
// + legacy purge. Confined behind `web` (Decision-3); the decisions live in the
// host-testable `prefs`/`shell` modules. The module form is the spec's
// sanctioned fallback to a separate `jeliya-platform-web` crate (§3.1).
#[cfg(feature = "web")]
pub mod platform_web;
#[cfg(feature = "ui")]
mod state;
// The truthful status display seam (#180 §6): every closed API vocabulary
// (Standing, Role, Liveness, Reachability, Link, Redeemability, Severity,
// StatusLabel, room-session Open/Closed) mapped to catalog copy by an
// exhaustive match, plus the absent/unknown fallbacks. A sibling of
// `l10n::wire` for the typed enums.
#[cfg(feature = "ui")]
pub mod status;
// The pure, host-testable view-model folds for the #180 product surfaces
// (People roster, invitations, room agents & runs, the global fleet, the
// capability gate, the Fleet poll machine, the LoadState six-state read, and
// the device-local alias map). No Dioxus and no platform `cfg`.
#[cfg(feature = "ui")]
pub mod view;

/// The pane-poll sleep (#180 §7.3/D7): target selection lives HERE at the
/// crate root (Decision-3), so shared panes stay free of `cfg`. The web build
/// sleeps on the browser's `setTimeout`; host builds (unit tests, the native
/// stub) park — host tests drive the poll machine directly and never wait on
/// a wall clock.
#[cfg(feature = "web")]
pub(crate) async fn poll_sleep(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

/// The host counterpart of the web `poll_sleep`: parks forever, so a poll
/// loop under test runs exactly one fetch and then quiesces.
#[cfg(all(feature = "ui", not(feature = "web")))]
pub(crate) async fn poll_sleep(_ms: u32) {
    futures::future::pending::<()>().await;
}

#[cfg(feature = "ui")]
pub use app::{AppRoot, AppRootProps};
// The canonical platform-authority contract lives in `jeliya-platform` (#174).
// `jeliya-ui` adopts it by re-export — the mechanical change #176 promised —
// so a component names `jeliya_ui::PlatformServices` exactly as before, now
// backed by the canonical crate. The former provisional `services.rs` seam and
// `WebPlatformServices` are deleted; composition injects a deterministic fake
// shape from `jeliya_platform::fake`.
#[cfg(feature = "ui")]
pub use jeliya_platform::PlatformServices;
#[cfg(feature = "ui")]
pub use state::UiState;

// Depend on `jeliya_api` for the typed operations, outputs, pushes, errors,
// ids, and shared value types, and on `jeliya_client` for the seam — this
// crate re-exports neither, to avoid a second spelling.
