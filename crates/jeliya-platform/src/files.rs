//! The files capability (#174 §D4/§D5, AC-1): safe types that distinguish the
//! platform object kinds a file surface touches, and the pick → stage → export
//! / open / share contract.
//!
//! The files capability never traffics in raw strings. Four distinct types
//! carry the four distinct object kinds:
//!
//! - [`PickedSource`] — an **opaque handle** to a user-selected *source*. It
//!   carries display metadata and a [`FileObjectKind`] discriminant, but the
//!   underlying path/URI is never reachable from a shared component: the
//!   component holds only an opaque [`SourceToken`] and the discriminant, so it
//!   cannot read a `content://` URI as a filesystem path because it can reach
//!   neither spelling. The service that produced the source keeps the internals
//!   in its own table, keyed by the token.
//! - [`ShareableBlob`] — a daemon-shareable handle produced **only** by
//!   [`Files::stage_for_share`]. A shared component cannot construct one, so the
//!   daemon's anti-exfiltration invariant is preserved at the type level (the
//!   UI cannot forge a daemon path).
//! - [`ExportTarget`] — a *destination* for a fetched file (a place to write),
//!   distinct from a [`PickedSource`] (a place to read).
//! - [`LocalFileRef`] — a typed reference to a daemon-fetched local copy,
//!   `(RoomId, FileId)`. The service resolves it to a token-carrying URL
//!   internally; components never see the URL or the token (§K5).

use jeliya_api::{FileId, RoomId};

use crate::cancel::CancelToken;
use crate::clipboard::ShareContent;
use crate::error::CapabilityError;
use crate::BoxFuture;

/// A user-facing display name for a picked or exported file. A newtype, not a
/// path: it is meant to be *shown*, and carries no directory information.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FileName(String);

impl FileName {
    /// Wrap a display name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The display name as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A MIME type reported for a picked source. A newtype so a component cannot
/// confuse it with arbitrary text.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Mime(String);

impl Mime {
    /// Wrap a MIME type string (for example `image/png`).
    pub fn new(mime: impl Into<String>) -> Self {
        Self(mime.into())
    }

    /// The MIME type as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The kind of platform object a [`PickedSource`] wraps — the **public
/// discriminant** of an object whose spelling stays private.
///
/// A component may branch on the kind (to label a source, say) but can never
/// obtain the underlying path or URI, which is the type-level enforcement of
/// "a local file path and a `content://` URI are not interchangeable".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileObjectKind {
    /// A browser `File`/blob reference — bytes reachable only via the platform,
    /// never a path.
    BrowserBlob,
    /// A desktop filesystem path (held privately in the producing service).
    NativePath,
    /// An Android `content://` URI (held privately in the producing service).
    ContentUri,
}

/// The kind of destination an [`ExportTarget`] wraps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExportTargetKind {
    /// A browser download — a suggested filename; the browser owns the
    /// destination.
    BrowserDownload,
    /// A desktop save-dialog path.
    NativePath,
    /// An Android SAF document `content://` write target.
    AndroidDocument,
}

/// An opaque handle to the internals of a [`PickedSource`]. Meaningless to a
/// component; the producing service maps it to the real path/URI/blob.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SourceToken(u64);

impl SourceToken {
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }

    pub(crate) fn get(self) -> u64 {
        self.0
    }
}

/// An opaque handle to the internals of an [`ExportTarget`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ExportToken(u64);

impl ExportToken {
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }

    pub(crate) fn get(self) -> u64 {
        self.0
    }
}

/// An opaque handle to a staged, daemon-shareable blob.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BlobToken(u64);

impl BlobToken {
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }

    pub(crate) fn get(self) -> u64 {
        self.0
    }
}

/// An opaque handle to a user-selected file source, produced by
/// [`Files::pick`].
///
/// Carries display metadata and a [`FileObjectKind`] discriminant. The
/// underlying path/URI/blob is **not** reachable from here: a component can
/// read [`PickedSource::kind`], [`PickedSource::display_name`],
/// [`PickedSource::size`], and [`PickedSource::mime`], but not a path — which is
/// how "a local file path and a `content://` URI are not interchangeable" is
/// enforced at the type level.
#[derive(Clone, Debug)]
pub struct PickedSource {
    token: SourceToken,
    display_name: FileName,
    size: u64,
    mime: Option<Mime>,
    kind: FileObjectKind,
}

impl PickedSource {
    pub(crate) fn new(
        token: SourceToken,
        display_name: FileName,
        size: u64,
        mime: Option<Mime>,
        kind: FileObjectKind,
    ) -> Self {
        Self {
            token,
            display_name,
            size,
            mime,
            kind,
        }
    }

    /// The display name of the selected source.
    pub fn display_name(&self) -> &FileName {
        &self.display_name
    }

    /// The selected source's size in bytes, as reported by the platform. For a
    /// streamed `content://` source this may be the platform's best estimate;
    /// [`Files::stage_for_share`] enforces the true size during the copy.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The reported MIME type, if any.
    pub fn mime(&self) -> Option<&Mime> {
        self.mime.as_ref()
    }

    /// The platform object kind — the only shape information a component may
    /// read.
    pub fn kind(&self) -> FileObjectKind {
        self.kind
    }

    pub(crate) fn token(&self) -> SourceToken {
        self.token
    }
}

/// A destination for a fetched file, produced by
/// [`Files::pick_export_target`].
///
/// Distinct from a [`PickedSource`]: an export target is a place to *write*, a
/// picked source is a place to *read*. As with a picked source, the underlying
/// destination path/URI is private; a component reads only the kind and the
/// suggested name.
#[derive(Clone, Debug)]
pub struct ExportTarget {
    token: ExportToken,
    kind: ExportTargetKind,
    suggested: FileName,
}

impl ExportTarget {
    pub(crate) fn new(token: ExportToken, kind: ExportTargetKind, suggested: FileName) -> Self {
        Self {
            token,
            kind,
            suggested,
        }
    }

    /// The kind of destination.
    pub fn kind(&self) -> ExportTargetKind {
        self.kind
    }

    /// The suggested filename for the export.
    pub fn suggested_name(&self) -> &FileName {
        &self.suggested
    }

    pub(crate) fn token(&self) -> ExportToken {
        self.token
    }
}

/// A daemon-shareable handle, produced **only** by [`Files::stage_for_share`].
///
/// It is the sole value the daemon's `file.share` operation accepts. A shared
/// component cannot construct one (the constructor is crate-private), so the
/// daemon's anti-exfiltration invariant — `file.share` refuses any path outside
/// the daemon data dir — is preserved at the type level: the UI cannot forge a
/// daemon path, because it holds only this opaque token.
#[derive(Clone)]
pub struct ShareableBlob {
    token: BlobToken,
    size: u64,
}

impl ShareableBlob {
    pub(crate) fn new(token: BlobToken, size: u64) -> Self {
        Self { token, size }
    }

    /// The staged blob's size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    pub(crate) fn token(&self) -> BlobToken {
        self.token
    }
}

impl PartialEq for ShareableBlob {
    fn eq(&self, other: &Self) -> bool {
        self.token == other.token
    }
}

impl std::fmt::Debug for ShareableBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Opaque: never render the staged path/name (§K1). Only the size and a
        // token discriminant, which carry no daemon path.
        f.debug_struct("ShareableBlob")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// A typed reference to a daemon-fetched local copy of a file, `(RoomId,
/// FileId)`.
///
/// [`Files::open_local`] and [`Files::export_local`] take this; the service
/// resolves it to the token-carrying `GET /api/files/local` URL internally, so
/// components never see the URL or the daemon token (§K5). This is why the
/// files capability takes a typed reference, not a URL string.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LocalFileRef {
    room_id: RoomId,
    file_id: FileId,
}

impl LocalFileRef {
    /// A reference to file `file_id` in room `room_id`.
    pub fn new(room_id: RoomId, file_id: FileId) -> Self {
        Self { room_id, file_id }
    }

    /// The room the file belongs to.
    pub fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    /// The file's id.
    pub fn file_id(&self) -> &FileId {
        &self.file_id
    }
}

/// A progress report during a staging copy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StageProgress {
    /// Bytes copied so far.
    pub transferred: u64,
    /// The total to copy, when known up front (a browser blob / desktop path);
    /// `None` for a streamed `content://` source whose size is not known until
    /// the stream ends.
    pub total: Option<u64>,
}

/// A sink for progress reports emitted by [`Files::stage_for_share`].
///
/// It wraps an optional callback. Use [`ProgressSink::discard`] when progress is
/// not needed. The callback runs synchronously on the staging task, so it must
/// not block; a deterministic test may use it to observe — or, to prove
/// mid-copy cancellation, to fire — a [`CancelToken`].
pub struct ProgressSink {
    report: Option<Box<dyn Fn(StageProgress)>>,
}

impl ProgressSink {
    /// A sink that invokes `report` for each progress update.
    pub fn new(report: impl Fn(StageProgress) + 'static) -> Self {
        Self {
            report: Some(Box::new(report)),
        }
    }

    /// A sink that ignores progress.
    pub fn discard() -> Self {
        Self { report: None }
    }

    /// Emit a progress update to the wrapped callback, if any.
    pub fn report(&self, progress: StageProgress) {
        if let Some(report) = &self.report {
            report(progress);
        }
    }
}

impl std::fmt::Debug for ProgressSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressSink").finish_non_exhaustive()
    }
}

/// The files capability: pick a source, stage it for the daemon to share,
/// export or open a daemon-fetched local copy, and share content.
///
/// Object-safe: every asynchronous method returns a [`BoxFuture`], so the
/// erased implementation stays behind `Arc<dyn Files>`.
pub trait Files {
    /// Open the platform file picker. Resolves to the selected
    /// [`PickedSource`], or `Ok(None)` **only** where the platform reports a
    /// clean no-selection that is not a user dismissal (rare). An actual user
    /// dismissal is [`CapabilityError::Cancelled`], kept distinct so a caller
    /// never treats a dismissed picker as "no files exist".
    fn pick(
        &self,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Option<PickedSource>, CapabilityError>>;

    /// Turn a [`PickedSource`] into a daemon-shareable [`ShareableBlob`].
    ///
    /// The copy is **bounded, size-enforced, and cancel-cleans-up**: the size
    /// is enforced against the daemon-reported `limit` (from `jeliya_api`'s
    /// served limits — never a constant redefined here). A known-oversize
    /// source fails [`FailureKind::FileTooLarge`](crate::FailureKind::FileTooLarge)
    /// *before any copy*; a streamed source is copied through a bounded buffer
    /// and aborts with `FileTooLarge` the instant the running total would
    /// exceed `limit`. Zero bytes fails
    /// [`FileEmpty`](crate::FailureKind::FileEmpty). On cancel or any failure
    /// the partial staged file is deleted and the outcome is `Cancelled` /
    /// `Failed`, **never** `Ok` — a failed or cancelled share must not leak
    /// bytes into the data dir.
    fn stage_for_share(
        &self,
        src: PickedSource,
        limit: u64,
        progress: ProgressSink,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<ShareableBlob, CapabilityError>>;

    /// Open the platform save dialog for a fetched file, suggesting
    /// `suggested`. `Ok(None)` / `Cancelled` follow the same distinction as
    /// [`Files::pick`].
    fn pick_export_target(
        &self,
        suggested: FileName,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Option<ExportTarget>, CapabilityError>>;

    /// Export a daemon-fetched local copy to a chosen [`ExportTarget`]. The
    /// service resolves the [`LocalFileRef`] to its token-carrying URL
    /// internally (§K5).
    fn export_local(
        &self,
        file: LocalFileRef,
        to: ExportTarget,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>>;

    /// Open a daemon-fetched local copy with the platform opener (a tab, the OS
    /// viewer). The service resolves the reference to its token-carrying URL
    /// internally (§K5).
    fn open_local(&self, file: LocalFileRef) -> BoxFuture<'_, Result<(), CapabilityError>>;

    /// Share content through the platform (the OS share sheet / a file share).
    /// Dismissing the sheet is [`CapabilityError::Cancelled`], not `Ok`.
    fn share_content(
        &self,
        content: ShareContent,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>>;
}
