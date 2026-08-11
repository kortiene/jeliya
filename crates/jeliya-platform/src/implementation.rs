//! The implementation-facing factory surface (feature `implementation`) for
//! the M3–M5 target crates.
//!
//! Target implementations live in **separate crates** (that is the point of
//! this crate's dependency isolation — see the crate docs), so they cannot
//! reach the crate-private constructors the fakes use. This module gives them
//! exactly the constructors and accessors an out-of-crate [`crate::Files`] /
//! [`crate::Share`] implementation needs — and nothing a shared component may
//! hold: the feature is default-off, `jeliya-ui` never enables it (asserted in
//! `tests/boundaries.rs`), so the shared-component graph still cannot forge a
//! [`ShareableBlob`] or read a token.
//!
//! **Anti-forgery is not lost by these factories.** No factory takes a path or
//! URI: the implementation keeps every spelling in its own private table,
//! keyed by the token it mints, and a token the producing service did not mint
//! fails closed at resolution (the fakes already enforce exactly this through
//! their minted-token registries). What the type system guarantees — no path
//! spelling is reachable from a shared component — survives unchanged.
//!
//! The whole module is compiled only under the `implementation` feature; the
//! feature gate lives at the crate root (`lib.rs`), keeping the contract
//! surface itself free of feature forks (§K10).

use crate::files::{
    BlobToken, ExportTarget, ExportTargetKind, ExportToken, FileName, FileObjectKind, Mime,
    PickedSource, ShareableBlob, SourceToken,
};

impl SourceToken {
    /// Wrap a raw token an implementation minted. The value is only a key
    /// into the implementation's private source table — it carries no
    /// path/URI and resolves nowhere else.
    pub fn from_raw(raw: u64) -> Self {
        Self::new(raw)
    }

    /// The raw token value, for keying the implementation's private table.
    pub fn into_raw(self) -> u64 {
        self.get()
    }
}

impl ExportToken {
    /// Wrap a raw token an implementation minted (see
    /// [`SourceToken::from_raw`]).
    pub fn from_raw(raw: u64) -> Self {
        Self::new(raw)
    }

    /// The raw token value, for keying the implementation's private table.
    pub fn into_raw(self) -> u64 {
        self.get()
    }
}

impl BlobToken {
    /// Wrap a raw token an implementation minted (see
    /// [`SourceToken::from_raw`]).
    pub fn from_raw(raw: u64) -> Self {
        Self::new(raw)
    }

    /// The raw token value, for keying the implementation's private table.
    pub fn into_raw(self) -> u64 {
        self.get()
    }
}

impl PickedSource {
    /// Construct a picked source from an implementation's minted token and
    /// display metadata. Takes no path/URI: the implementation keeps the
    /// spelling in its own private table keyed by `token`, and an unminted
    /// token fails closed when the source is later staged.
    pub fn for_implementation(
        token: SourceToken,
        display_name: FileName,
        size: u64,
        mime: Option<Mime>,
        kind: FileObjectKind,
    ) -> Self {
        Self::new(token, display_name, size, mime, kind)
    }

    /// The token this source was minted with — for the *producing* service to
    /// resolve on the inbound side of
    /// [`Files::stage_for_share`](crate::Files::stage_for_share).
    pub fn source_token(&self) -> SourceToken {
        self.token()
    }
}

impl ExportTarget {
    /// Construct an export target from an implementation's minted token, its
    /// kind, and the suggested name. Path-free, like every factory here.
    pub fn for_implementation(
        token: ExportToken,
        kind: ExportTargetKind,
        suggested: FileName,
    ) -> Self {
        Self::new(token, kind, suggested)
    }

    /// The token this target was minted with — for the producing service to
    /// resolve (and consume) in
    /// [`Files::export_sink`](crate::Files::export_sink).
    pub fn export_token(&self) -> ExportToken {
        self.token()
    }
}

impl ShareableBlob {
    /// Construct a staged-blob handle from an implementation's minted token
    /// and the staged size. Path-free: the staging location stays in the
    /// implementation's private table, and a blob the service did not stage
    /// fails closed in [`Files::read_staged`](crate::Files::read_staged) and
    /// on share.
    pub fn for_implementation(token: BlobToken, size: u64) -> Self {
        Self::new(token, size)
    }

    /// The token this blob was minted with — for the producing service to
    /// resolve in [`Files::read_staged`](crate::Files::read_staged) and
    /// share operations.
    pub fn blob_token(&self) -> BlobToken {
        self.token()
    }
}
