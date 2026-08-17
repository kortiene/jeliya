//! The browser (wasm32) target implementation of `jeliya-platform`'s file/share
//! capabilities (#181), and the **door-edge home** the architecture record wants
//! (Decision-3/-4/-7, #174 §K4, #181 D0).
//!
//! Introducing a real `Files` implementation forces a dependency edge to the
//! `jeliya-platform-implementation` factory door (the only way to mint
//! `PickedSource`/`ShareableBlob`/`ExportTarget`/`FetchedArtifact`). K4 forbids
//! that edge inside the *shared* `jeliya-ui` component graph, so it lives here
//! instead: this crate depends on the door, the `web`-gated composition assembles
//! these bindings into `WebPlatform`, and the default `jeliya-ui` graph gains no
//! door edge (asserted by `jeliya-platform`'s `tests/boundaries.rs` allowlist).
//!
//! The [`registry`] — the fail-closed minted-token gate that backs anti-forgery
//! at runtime (the guarantee that survives Cargo feature unification, §K4) — is
//! portable and host-tested. The `web-sys` bindings ([`files`], [`share`],
//! [`launcher`]) compile only for `wasm32`; web-sys is proven only in-browser,
//! never on the host.
//!
//! Where this crate and `docs/dioxus-architecture.md` / `docs/protocol-v2.md` /
//! the #174 contract disagree, the record is right and this crate has a bug —
//! say which in the PR.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod registry;

#[cfg(target_arch = "wasm32")]
pub mod files;
#[cfg(target_arch = "wasm32")]
pub mod launcher;
#[cfg(target_arch = "wasm32")]
pub mod share;

#[cfg(target_arch = "wasm32")]
pub use files::WebFiles;
#[cfg(target_arch = "wasm32")]
pub use launcher::WebUrlLauncher;
#[cfg(target_arch = "wasm32")]
pub use share::{WebClipboard, WebShare};
