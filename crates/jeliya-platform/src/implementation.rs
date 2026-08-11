//! The implementation-facing factory surface (feature `implementation`) for
//! the M3–M5 target crates.
//!
//! Target implementations live in **separate crates** (that is the point of
//! this crate's dependency isolation — see the crate docs), so they cannot
//! reach the crate-private constructors the fakes use. This module gives them
//! exactly the constructors and accessors an out-of-crate [`crate::Files`] /
//! [`crate::Share`] implementation needs — and nothing more.
//!
//! # Reaching this module
//!
//! A target crate depends on **`jeliya-platform-implementation`**, the one
//! blessed door: it is the only manifest in the workspace permitted to write
//! `jeliya-platform = { …, features = ["implementation"] }`, and
//! `tests/boundaries.rs` scans every other member to keep it that way. Depend
//! on `jeliya-platform` (default features) for the contract, and on the door
//! crate for these factories.
//!
//! # Why free functions, not inherent methods
//!
//! Cargo **unifies features per package across a build graph**, so once any
//! crate in a target binary enables `implementation`, this module is compiled
//! into the single `jeliya-platform` instance the shared UI also links. An
//! inherent method would then be reachable from shared code by bare method
//! syntax (`BlobToken::from_raw(7)`) with nothing to grep for. These are free
//! functions instead: every call site must spell the `implementation` path
//! segment, which is exactly the one token the workspace source scan rejects
//! outside the door crate. What unification cannot touch is a **dependency
//! edge**, so the door crate's edge is asserted separately by a `cargo tree`
//! test.
//!
//! # Anti-forgery is not lost by these factories
//!
//! No factory takes a path or URI: the implementation keeps every spelling in
//! its own private table, keyed by the token it mints, and a token the
//! producing service did not mint **fails closed at resolution** (the fakes
//! enforce exactly this through their minted-token registries, whose ids carry
//! issuer provenance). What the type system guarantees — no path spelling is
//! reachable from a shared component — survives unification unchanged, and the
//! runtime gate is what holds when the compile-time absence does not.
//!
//! The whole module is compiled only under the `implementation` feature; the
//! feature gate lives at the crate root (`lib.rs`), keeping the contract
//! surface itself free of feature forks (§K10).

use crate::files::{
    ArtifactToken, BlobToken, ExportTarget, ExportTargetKind, ExportToken, FetchedArtifact,
    FileName, FileObjectKind, Mime, PickedSource, ShareableBlob, SourceToken,
};

/// Wrap a raw source token an implementation minted. The value is only a key
/// into the implementation's private source table — it carries no path/URI and
/// resolves nowhere else.
pub fn source_token_from_raw(raw: u64) -> SourceToken {
    SourceToken::new(raw)
}

/// The raw source-token value, for keying the implementation's private table.
pub fn source_token_into_raw(token: SourceToken) -> u64 {
    token.get()
}

/// Wrap a raw export token an implementation minted (see
/// [`source_token_from_raw`]).
pub fn export_token_from_raw(raw: u64) -> ExportToken {
    ExportToken::new(raw)
}

/// The raw export-token value, for keying the implementation's private table.
pub fn export_token_into_raw(token: ExportToken) -> u64 {
    token.get()
}

/// Wrap a raw blob token an implementation minted (see
/// [`source_token_from_raw`]).
pub fn blob_token_from_raw(raw: u64) -> BlobToken {
    BlobToken::new(raw)
}

/// The raw blob-token value, for keying the implementation's private table.
pub fn blob_token_into_raw(token: BlobToken) -> u64 {
    token.get()
}

/// Wrap a raw artifact token an implementation minted (see
/// [`source_token_from_raw`]).
pub fn artifact_token_from_raw(raw: u64) -> ArtifactToken {
    ArtifactToken::new(raw)
}

/// The raw artifact-token value, for keying the implementation's private table.
pub fn artifact_token_into_raw(token: ArtifactToken) -> u64 {
    token.get()
}

/// Construct a picked source from an implementation's minted token and display
/// metadata. Takes no path/URI: the implementation keeps the spelling in its
/// own private table keyed by `token`, and an unminted token fails closed when
/// the source is later staged.
///
/// `display_name` is a validated [`FileName`]: a genuine platform name may
/// carry separators or control characters (Linux permits both), so the
/// implementation sanitizes it before [`FileName::parse`] — construction fails
/// closed rather than normalizing silently.
pub fn picked_source(
    token: SourceToken,
    display_name: FileName,
    size: u64,
    mime: Option<Mime>,
    kind: FileObjectKind,
) -> PickedSource {
    PickedSource::new(token, display_name, size, mime, kind)
}

/// The token a source was minted with — for the *producing* service to resolve
/// on the inbound side of
/// [`Files::stage_for_share`](crate::Files::stage_for_share).
pub fn picked_source_token(source: &PickedSource) -> SourceToken {
    source.token()
}

/// Construct an export target from an implementation's minted token, its kind,
/// and the suggested name. Path-free, like every factory here; `suggested` is a
/// validated [`FileName`] the implementation sanitized before
/// [`FileName::parse`] (see [`picked_source`]).
pub fn export_target(
    token: ExportToken,
    kind: ExportTargetKind,
    suggested: FileName,
) -> ExportTarget {
    ExportTarget::new(token, kind, suggested)
}

/// The token an export target was minted with — for the producing service to
/// resolve (and consume) in [`Files::export_sink`](crate::Files::export_sink).
pub fn export_target_token(target: &ExportTarget) -> ExportToken {
    target.token()
}

/// Construct a staged-blob handle from an implementation's minted token and the
/// staged size. Path-free: the staging location stays in the implementation's
/// private table, and a blob the service did not stage fails closed in
/// [`Files::read_staged`](crate::Files::read_staged) and on share.
pub fn shareable_blob(token: BlobToken, size: u64) -> ShareableBlob {
    ShareableBlob::new(token, size)
}

/// The token a blob was minted with — for the producing service to resolve in
/// [`Files::read_staged`](crate::Files::read_staged) and share operations.
pub fn shareable_blob_token(blob: &ShareableBlob) -> BlobToken {
    blob.token()
}

/// Construct a fetched-artifact handle from an implementation's minted token,
/// the display name, and the materialized size — what
/// [`ShareSink::commit`](crate::files::ShareSink::commit) returns. Path-free:
/// the staging location stays in the implementation's private table, and an
/// artifact the service did not materialize fails closed when the share sheet
/// is presented.
pub fn fetched_artifact(token: ArtifactToken, name: FileName, size: u64) -> FetchedArtifact {
    FetchedArtifact::new(token, name, size)
}

/// The token an artifact was minted with — for the producing service to resolve
/// (and consume on a successful share) in
/// [`Files::share_content`](crate::Files::share_content) /
/// [`Share::share`](crate::Share::share).
pub fn fetched_artifact_token(artifact: &FetchedArtifact) -> ArtifactToken {
    artifact.token()
}
