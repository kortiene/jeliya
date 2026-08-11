//! The single blessed door to [`jeliya_platform`]'s implementation-facing
//! factory surface (#174 §K4).
//!
//! # What this crate is for
//!
//! The M3–M5 target implementations (browser `web-sys`, desktop file dialogs,
//! Android SAF/JNI) live in their own crates, so they cannot reach the
//! crate-private constructors `jeliya-platform`'s fakes use. They need the
//! path-free factories in `jeliya_platform::implementation` to turn tokens they
//! mint into [`jeliya_platform::PickedSource`],
//! [`jeliya_platform::ExportTarget`], [`jeliya_platform::ShareableBlob`], and
//! [`jeliya_platform::FetchedArtifact`]. A target crate therefore depends on
//! **`jeliya-platform`** (default features) for the contract and on **this
//! crate** for the factories.
//!
//! # Why a separate crate rather than "just enable the feature"
//!
//! Cargo [unifies features per package across a build
//! graph](https://doc.rust-lang.org/cargo/reference/features.html#feature-unification).
//! The moment any crate in a target binary enables `jeliya-platform`'s
//! `implementation` feature, the factory module is compiled into the **single**
//! `jeliya-platform` instance every crate in that binary links — including the
//! one the shared `jeliya-ui` uses. A default-off feature is therefore *not* a
//! boundary in a real target build: the shared UI would gain the factory
//! surface as a side effect of some other crate's dependency choice.
//!
//! A **dependency edge does not unify.** So the boundary is relocated to one:
//! this crate is the only manifest permitted to enable the feature, and
//! `jeliya-platform`'s `tests/boundaries.rs` enforces exactly that —
//!
//! - a workspace-wide manifest scan rejects any non-allowlisted member that
//!   names the feature or this crate;
//! - a source scan rejects any shared crate that spells the `implementation`
//!   path (which is why those factories are free functions, not inherent
//!   methods: a call site cannot reach them without naming the path);
//! - a `cargo tree` test asserts the shared UI graph carries no edge to this
//!   crate.
//!
//! Admitting a new crate to the factory surface means adding it to that
//! allowlist — a deliberate, reviewable edit, which is the point.
//!
//! # What this crate does not do
//!
//! It adds no behaviour and no types of its own; it is a re-export and a
//! boundary marker. Anti-forgery does not rest on it alone: every factory here
//! is path-free, and a token the producing service did not mint fails closed at
//! resolution, so a forged handle resolves nowhere even inside a target binary.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub use jeliya_platform::implementation::*;
