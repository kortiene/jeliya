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
//!   [`Files::stage_for_share`]. A shared component cannot construct one under
//!   default features, so the daemon's anti-exfiltration invariant is
//!   preserved at the type level (the UI cannot forge a daemon path). The
//!   M3–M5 target crates construct these through the path-free factories
//!   behind the default-off `implementation` feature; a token the producing
//!   service did not mint fails closed at resolution.
//! - [`ExportTarget`] — a *destination* for a fetched file (a place to write),
//!   distinct from a [`PickedSource`] (a place to read).
//! - [`LocalFileRef`] — a typed reference to a room file, `(RoomId, FileId)` —
//!   the identifier pair the UI feeds to the client seam's `file.read` and the
//!   only file identity [`crate::ShareAttachment::Local`] carries. In protocol
//!   v2 file bytes travel the byte-stream framing (`docs/protocol-v2.md`
//!   §file.read), not an HTTP URL, so no token-carrying URL exists to resolve;
//!   the daemon-token half of §K5 is enforced by
//!   [`crate::storage::SecretStore`] custody alone.
//!
//! Bytes cross this boundary through two small object-safe traits, never
//! through paths: [`StagedBlobReader`] (the **pull** side — the v2 upload
//! stream pulls exactly what CREDIT permits from a staged blob) and
//! [`FileSink`] (the **push** side — the UI pumps `file.read` DATA chunks into
//! a platform destination). Only bytes and identifiers cross; every spelling
//! stays inside the producing service.

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
/// component cannot construct one (the constructor is crate-private under
/// default features; target implementation crates use the path-free
/// `for_implementation` factory behind the `implementation` feature), so the
/// daemon's anti-exfiltration invariant — `file.share` refuses any path outside
/// the daemon data dir — is preserved at the type level: the UI cannot forge a
/// daemon path, because it holds only this opaque token, and a token the
/// producing service did not mint fails closed at resolution.
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

/// A typed reference to a room file, `(RoomId, FileId)`.
///
/// This is the identifier pair the UI hands to the client seam's `file.read`
/// operation and the only file identity [`crate::ShareAttachment::Local`]
/// carries. It is deliberately **not** a URL: in protocol v2 the retired
/// `GET /api/files/local` HTTP edge does not exist — file bytes travel the
/// byte-stream framing, pumped into a [`FileSink`] from
/// [`Files::export_sink`] / [`Files::open_sink`] — so there is no
/// token-carrying URL to resolve and no daemon token near this type (§K5).
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

/// A bounded, pull-shaped reader over a staged blob's bytes, produced by
/// [`Files::read_staged`] — the platform half of the v2 `file.share` upload.
///
/// **Pull-shaped on purpose:** the v2 upload stream sends Binary DATA frames
/// only under receiver-driven CREDIT (`docs/protocol-v2.md` §Byte-stream
/// framing), so the uploader pulls one bounded chunk per DATA frame it is
/// permitted to send — the reader never pushes bytes the stream has no credit
/// for. The trait is object-safe and its future is the crate's `!Send`
/// [`BoxFuture`] (browser platform handles are not `Send`); the client-side
/// upload input that consumes this must therefore accept `!Send` sources.
pub trait StagedBlobReader {
    /// Total bytes in the staged blob. Equals the blob's staged size and
    /// feeds the `file.share` request's `declared_bytes`.
    fn size(&self) -> u64;

    /// The next chunk, at most `max_len` bytes. `Ok(None)` is clean EOF;
    /// [`FailureKind::Unreadable`](crate::FailureKind::Unreadable) if the
    /// staged bytes vanished mid-read (the staged file was reaped).
    fn next_chunk(
        &mut self,
        max_len: usize,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, CapabilityError>>;
}

/// A push-shaped byte destination for a fetched file, produced by
/// [`Files::export_sink`] / [`Files::open_sink`] — the platform half of the
/// v2 `file.read` download.
///
/// The UI drives the client seam's `file.read` and pumps each DATA chunk in;
/// **the `write` future resolving is the platform's acceptance signal**: the
/// caller advances its `file.read` CREDIT only after the chunk was accepted
/// (protocol rule: the receiver advances credit only as its sink accepts,
/// `docs/protocol-v2.md` §Byte-stream framing), so a slow disk or a stalled
/// SAF document naturally throttles the stream instead of buffering it.
///
/// **Drop is abort (§D12/K2):** dropping an uncommitted sink aborts the write
/// and deletes the partial artifact — the same cancel-cleans-up honesty
/// [`Files::stage_for_share`] has. Only [`FileSink::commit`] makes the bytes
/// real.
pub trait FileSink {
    /// Append one chunk. Resolution is acceptance — advance stream credit
    /// only after it resolves `Ok`.
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), CapabilityError>>;

    /// Finalize the artifact (complete the browser download, close the SAF
    /// document, publish the temp file). For a sink from
    /// [`Files::open_sink`], commit also invokes the platform opener on the
    /// finished artifact.
    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<(), CapabilityError>>;
}

/// The files capability: pick a source, stage it for the daemon to share,
/// read staged bytes for upload, sink fetched bytes to an export target or
/// the platform opener, and share content.
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

    /// Open a bounded [`StagedBlobReader`] over a staged blob's bytes, for
    /// the v2 `file.share` upload to pull under CREDIT.
    ///
    /// Token resolution stays inside the service: a blob this service did not
    /// stage fails
    /// [`FailureKind::Unreadable`](crate::FailureKind::Unreadable) — never an
    /// empty `Ok` reader. Paths never cross; only bytes do.
    fn read_staged(
        &self,
        blob: &ShareableBlob,
    ) -> BoxFuture<'_, Result<Box<dyn StagedBlobReader>, CapabilityError>>;

    /// Open the platform save dialog for a fetched file, suggesting
    /// `suggested`. `Ok(None)` / `Cancelled` follow the same distinction as
    /// [`Files::pick`].
    fn pick_export_target(
        &self,
        suggested: FileName,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Option<ExportTarget>, CapabilityError>>;

    /// Open a [`FileSink`] writing to a chosen [`ExportTarget`]. The UI
    /// drives the client seam's `file.read` and pumps the DATA chunks in;
    /// this method consumes the target (a re-used target fails closed). The
    /// destination spelling stays inside the service — the sink is bytes-only.
    fn export_sink(
        &self,
        to: ExportTarget,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Box<dyn FileSink>, CapabilityError>>;

    /// Open a [`FileSink`] whose [`FileSink::commit`] hands the finished
    /// artifact to the platform opener (an object-URL tab on the browser, the
    /// OS viewer on desktop). `declared` is the peer-declared content type —
    /// an **untrusted opener hint, never a trust decision**
    /// (`docs/protocol-v2.md` §file.read: never render inline on that
    /// declaration).
    fn open_sink(
        &self,
        name: FileName,
        declared: Option<Mime>,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Box<dyn FileSink>, CapabilityError>>;

    /// Share content through the platform (the OS share sheet / a file share).
    /// Dismissing the sheet is [`CapabilityError::Cancelled`], not `Ok`.
    fn share_content(
        &self,
        content: ShareContent,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>>;
}
