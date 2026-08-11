//! The files capability (#174 §D4/§D5, AC-1): safe types that distinguish the
//! platform object kinds a file surface touches, and the pick → stage → export
//! / open / share contract.
//!
//! The files capability never traffics in raw strings. Five distinct types
//! carry the distinct object kinds a file surface touches:
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
//!   M3–M5 target crates construct these through the path-free factories the
//!   `jeliya-platform-implementation` door crate re-exports; a token the
//!   producing service did not mint fails closed at resolution, which is what
//!   holds even where Cargo feature unification compiles that surface in.
//! - [`ExportTarget`] — a *destination* for a fetched file (a place to write),
//!   distinct from a [`PickedSource`] (a place to read).
//! - [`LocalFileRef`] — a typed reference to a room file, `(RoomId, FileId)` —
//!   the identifier pair the UI feeds to the client seam's `file.read`. In
//!   protocol v2 file bytes travel the byte-stream framing
//!   (`docs/protocol-v2.md` §file.read), not an HTTP URL, so no token-carrying
//!   URL exists to resolve; the daemon-token half of §K5 is enforced by
//!   [`crate::storage::SecretStore`] custody alone. It is an *identity*, not an
//!   attachment: sharing a room file means pumping its bytes through
//!   [`Files::share_sink`] into the service's own custody first.
//! - [`FetchedArtifact`] — the counterpart of a [`ShareableBlob`] on the
//!   *inbound* side: a handle to bytes the service itself custodies after the
//!   UI pumped a `file.read` stream into a [`ShareSink`], and the only file
//!   identity [`crate::ShareAttachment::Fetched`] carries.
//!
//! Bytes cross this boundary through three small object-safe traits, never
//! through paths: [`StagedBlobReader`] (the **pull** side — the v2 upload
//! stream pulls exactly what CREDIT permits from a staged blob), [`FileSink`]
//! (the **push** side — the UI pumps `file.read` DATA chunks into a platform
//! destination), and [`ShareSink`] (the same push shape, but committing into
//! the *service's own* staging custody so the share sheet has a real artifact
//! to offer). Only bytes and identifiers cross; every spelling stays inside the
//! producing service.

use jeliya_api::{FileId, RoomId};

use crate::cancel::CancelToken;
use crate::clipboard::ShareContent;
use crate::error::CapabilityError;
use crate::BoxFuture;

/// A user-facing display name for a picked or exported file. A newtype, not a
/// path: it is meant to be *shown*, and carries no directory information.
///
/// The invariant is **enforced, not promised**: [`FileName::parse`] is the only
/// constructor, and the value it admits is a non-empty single path component —
/// no `/` or `\` separator, not `.` or `..`, no control characters. What is
/// enforced is *portable path syntax*: the name cannot carry a separator or a
/// navigation component into a sink's artifact naming.
///
/// It is **not** a substitute for the sink's own platform naming rules, and two
/// of those are directory-affecting **on Windows**, so a Windows sink must
/// sanitize before it joins:
///
/// - no `:` filtering — `C:name` is an ordinary POSIX file name but a
///   *drive-qualified* path on Windows, and `Path::join`/`PathBuf::push`
///   replace the whole base when the pushed path "has a prefix but no root", so
///   joining it escapes the owned directory entirely;
/// - no trailing-dot or trailing-space trimming — Windows strips these during
///   path normalization, so `".. "` normalizes to a navigation component.
///
/// The rest are naming rules rather than traversal, and are likewise the sink's:
///
/// - no Unicode normalization (NFC/NFD) or case folding;
/// - no Windows reserved-device-name filtering (`CON`, `NUL`, `AUX`, …);
/// - no length cap (filesystems differ);
/// - no hidden-file policy (a leading `.` is a legal name).
///
/// So: a **POSIX** sink may join a `FileName` under a directory it owns without
/// re-checking for traversal; a **Windows** sink must first reject or rewrite a
/// name containing `:` and trim trailing dots and spaces. The type deliberately
/// does neither, because `10:30 notes.txt` is an ordinary file name on the
/// targets that are actually committed (`docs/platform-matrix.md`), and
/// rejecting it here would make a peer's file unopenable and unshareable there
/// to close a hazard on a target whose scope is still undecided.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FileName(String);

/// Why a string was rejected as a [`FileName`].
///
/// The rejected string is **not** carried (§K1, mirroring
/// [`UnsafeUrl`](crate::launcher::UnsafeUrl)): a peer-supplied name that failed
/// validation must not leak into a log or error string.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
pub enum InvalidFileName {
    /// The name was empty.
    #[error("file name is empty")]
    Empty,
    /// The name contained a `/` or `\` path separator.
    #[error("file name contains a path separator")]
    Separator,
    /// The whole name was `.` or `..` — a path-navigation component.
    #[error("file name is a path-navigation component")]
    PathComponent,
    /// The name contained a control character (NUL, C0, DEL, or C1).
    #[error("file name contains a control character")]
    Control,
}

impl FileName {
    /// Parse and validate a display name, **failing closed**: a peer- or
    /// caller-provided name that carries path syntax is rejected rather than
    /// normalized, so no caller can be surprised by a silently rewritten name.
    ///
    /// Rejects an empty name, the navigation components `.` and `..`, any name
    /// containing `/` or `\`, and any name containing a control character. See
    /// the type docs for what validation deliberately leaves to the platform.
    pub fn parse(name: impl Into<String>) -> Result<Self, InvalidFileName> {
        let name = name.into();
        if name.is_empty() {
            return Err(InvalidFileName::Empty);
        }
        // Whole-name navigation only: `a..b` and `..hidden` are ordinary names.
        if name == "." || name == ".." {
            return Err(InvalidFileName::PathComponent);
        }
        if name.contains(['/', '\\']) {
            return Err(InvalidFileName::Separator);
        }
        if name.chars().any(char::is_control) {
            return Err(InvalidFileName::Control);
        }
        Ok(Self(name))
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

/// An opaque handle to a fetched artifact the service custodies for sharing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ArtifactToken(u64);

impl ArtifactToken {
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
/// It is the sole value the daemon's `file.share` operation accepts, so the
/// daemon's anti-exfiltration invariant — `file.share` refuses any path outside
/// the daemon data dir — is preserved at the type level: the UI cannot forge a
/// daemon path, because it holds only this opaque token.
///
/// The constructor is crate-private; target implementation crates reach the
/// path-free factory through `jeliya-platform-implementation`. The re-export of
/// this type at the crate root states in two tiers what that boundary does and
/// does not guarantee once Cargo unifies features in a target binary — in
/// short, a token the producing service did not mint **fails closed at
/// resolution**, which is the guarantee that survives unification.
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

/// A handle to a **fetched** room file the service now custodies, produced
/// **only** by committing a [`ShareSink`] from [`Files::share_sink`].
///
/// It is the inbound mirror of [`ShareableBlob`]: where a shareable blob is the
/// one value the *daemon's* `file.share` accepts, a fetched artifact is the one
/// value the *platform's* share sheet accepts for a room file
/// ([`crate::ShareAttachment::Fetched`]). The two are deliberately distinct
/// types so the directions cannot be crossed — a fetched artifact must never
/// flow back into [`Files::read_staged`] or the daemon's share.
///
/// The artifact lives in the service's private staging area (an Android
/// protected staging dir, a desktop temp staging dir, a browser in-memory
/// blob); the location is never reachable from here. A successful share
/// consumes it — "delete after share", the same reaping discipline staging has
/// — so a re-share of the same handle fails closed rather than offering bytes
/// the service no longer owns.
#[derive(Clone)]
pub struct FetchedArtifact {
    token: ArtifactToken,
    name: FileName,
    size: u64,
}

impl FetchedArtifact {
    pub(crate) fn new(token: ArtifactToken, name: FileName, size: u64) -> Self {
        Self { token, name, size }
    }

    /// The display name the artifact was materialized under.
    pub fn name(&self) -> &FileName {
        &self.name
    }

    /// The artifact's size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    pub(crate) fn token(&self) -> ArtifactToken {
        self.token
    }
}

impl PartialEq for FetchedArtifact {
    fn eq(&self, other: &Self) -> bool {
        self.token == other.token
    }
}

impl std::fmt::Debug for FetchedArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Opaque like `ShareableBlob`: the staging location never renders
        // (§K1). The display name is user-facing by construction, so it is
        // safe to show; the token and the path are not.
        f.debug_struct("FetchedArtifact")
            .field("name", &self.name)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// A typed reference to a room file, `(RoomId, FileId)`.
///
/// This is the identifier pair the UI hands to the client seam's `file.read`
/// operation. It is deliberately **not** a URL: in protocol v2 the retired
/// `GET /api/files/local` HTTP edge does not exist — file bytes travel the
/// byte-stream framing, pumped into a [`FileSink`] from
/// [`Files::export_sink`] / [`Files::open_sink`], or into a [`ShareSink`] from
/// [`Files::share_sink`] — so there is no token-carrying URL to resolve and no
/// daemon token near this type (§K5).
///
/// It is an identity, never an attachment: a [`crate::ShareContent`] carries a
/// [`FetchedArtifact`] whose bytes the service already holds, so the platform
/// never has to resolve an id it cannot read.
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
    ///
    /// `max_len` **must be nonzero**: a puller with no credit must not pull.
    /// Implementations reject a zero bound with
    /// [`FailureKind::Io`](crate::FailureKind::Io) rather than returning an
    /// empty chunk, so a zero bound can never masquerade as progress or as EOF
    /// — an adapter deriving the bound from currently available credit would
    /// otherwise spin forever on empty non-EOF chunks. The read position is not
    /// consumed, so a later bounded pull still yields the correct next bytes.
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

/// A push-shaped byte destination that commits into the **service's own**
/// staging custody, produced by [`Files::share_sink`] — the platform half of
/// sharing a fetched room file.
///
/// Same push shape and same credit contract as [`FileSink`]: the UI drives the
/// client seam's `file.read` and pumps each DATA chunk in, advancing stream
/// credit only as each `write` resolves. It is a **separate trait** because the
/// destination is different in kind: a [`FileSink`] commits to a user-chosen
/// destination and yields nothing, while committing here yields a
/// [`FetchedArtifact`] the service can subsequently hand to the share sheet.
/// An export or open sink must never be able to fabricate one.
///
/// **Drop is abort (§D12/K2):** dropping an uncommitted share sink deletes the
/// partial artifact and mints no handle, exactly as [`FileSink`] and
/// [`Files::stage_for_share`] do.
pub trait ShareSink {
    /// Append one chunk. Resolution is acceptance — advance stream credit
    /// only after it resolves `Ok`.
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), CapabilityError>>;

    /// Materialize the bytes in the service's private staging area and mint the
    /// [`FetchedArtifact`] that names them.
    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<FetchedArtifact, CapabilityError>>;
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

    /// Release a [`PickedSource`] the caller will not stage — the picker half
    /// of the same custody rule [`Files::release_staged`] states for staging.
    ///
    /// A pick that succeeds hands the component an opaque token while the
    /// producing service keeps the real `File`, path, or `content://` grant in
    /// its private table. If the user then navigates away, nothing tells the
    /// service the object is dead, and repeated abandoned picks retain file
    /// objects and URI grants for the service's lifetime. A guard cannot solve
    /// it — [`PickedSource`] is `Clone`, so a `Drop` impl cannot know which
    /// copy was the last — which is why the release is explicit.
    ///
    /// Consuming the handle by value makes it final; a source this service did
    /// not produce, or one already staged or discarded, fails
    /// [`FailureKind::Unreadable`](crate::FailureKind::Unreadable).
    /// [`Files::stage_for_share`] consumes its source the same way, so a staged
    /// pick needs no discard.
    fn discard_source(&self, src: PickedSource) -> BoxFuture<'_, Result<(), CapabilityError>>;

    /// Release a [`FetchedArtifact`] that will not be shared — the abandoned
    /// counterpart of a successful share consuming it.
    ///
    /// A committed [`ShareSink`] leaves real bytes in the service's staging
    /// area (a browser blob, a native temporary file). Sharing consumes them,
    /// but a user who backs out of the sheet, or stops retrying after a
    /// cancelled or failed share, otherwise leaves them there for the service's
    /// lifetime. As with the other handles, `Clone` rules out a `Drop` guard —
    /// no copy knows it is the last — so the release is explicit and final;
    /// an artifact already shared or released fails
    /// [`FailureKind::Unreadable`](crate::FailureKind::Unreadable).
    fn release_artifact(
        &self,
        artifact: FetchedArtifact,
    ) -> BoxFuture<'_, Result<(), CapabilityError>>;

    /// Release an [`ExportTarget`] the caller will not write to — the same rule
    /// for the destination side, where an Android SAF create-document grant is
    /// the resource being held. [`Files::export_sink`] consumes the target, so
    /// only an abandoned one needs this.
    fn discard_export_target(
        &self,
        target: ExportTarget,
    ) -> BoxFuture<'_, Result<(), CapabilityError>>;

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
    ///
    /// A blob that *was* staged is the caller's to release: the bytes live
    /// until [`Files::release_staged`] is called, because only the UI knows
    /// whether the daemon's `file.share` settled.
    ///
    /// The **source** is consumed either way: it is taken by value and the
    /// service drops its private entry on *every* outcome — success, a typed
    /// failure, a cancellation, or the future being dropped — because no
    /// caller can reach [`Files::discard_source`] for a handle it has already
    /// moved in here. An abandoned pick is the only one that needs discarding.
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

    /// Release a staged blob's bytes once the daemon's `file.share` has
    /// **settled** — the outbound half of "delete after share" (§D5).
    ///
    /// The service cannot learn this on its own and must not guess: reaching
    /// EOF in a [`StagedBlobReader`] means the bytes were *read*, not that the
    /// daemon accepted the operation (a retry re-reads them), and dropping a
    /// [`ShareableBlob`] is invisible to the service because the handle is an
    /// opaque token, not a guard. The UI drives `file.share`, so the UI is the
    /// only party that knows the outcome; it calls this on **success or
    /// failure** once no further attempt will be made, and the service deletes
    /// its staging copy.
    ///
    /// Consuming the handle by value makes the release final: a blob this
    /// service did not stage, or one already released, fails
    /// [`FailureKind::Unreadable`](crate::FailureKind::Unreadable) — the same
    /// minted-token gate [`Files::read_staged`] applies. A reader already open
    /// over the bytes keeps working, mirroring an open file descriptor
    /// outliving an unlink.
    fn release_staged(&self, blob: ShareableBlob) -> BoxFuture<'_, Result<(), CapabilityError>>;

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

    /// Open a [`ShareSink`] for a room file the user wants to share — the
    /// platform half of sharing fetched bytes.
    ///
    /// The UI drives the client seam's `file.read` for a [`LocalFileRef`] and
    /// pumps the DATA chunks in under CREDIT; [`ShareSink::commit`]
    /// materializes them in the service's private staging area and returns the
    /// [`FetchedArtifact`] to attach as
    /// [`crate::ShareAttachment::Fetched`]. This is why a room file needs no
    /// URL and no client seam inside the platform service (§K5/§K11): the only
    /// thing that crosses is bytes the UI already had.
    ///
    /// `declared` is the peer-declared content type — an **untrusted hint** for
    /// the share sheet's type negotiation, never a trust decision, exactly as
    /// in [`Files::open_sink`].
    fn share_sink(
        &self,
        name: FileName,
        declared: Option<Mime>,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Box<dyn ShareSink>, CapabilityError>>;

    /// Share content through the platform (the OS share sheet / a file share).
    /// Dismissing the sheet is [`CapabilityError::Cancelled`], not `Ok`.
    ///
    /// Takes the content by **reference**: a denied, dismissed, or dropped
    /// share leaves the caller holding its attachment handle, which it needs in
    /// order to retry or to release. Consuming the content would strand the
    /// bytes — the service still owns them, but the only handle naming them
    /// would be gone. A [`ShareableBlob`] is not consumed by sharing at all
    /// (`release_staged` reaps it); a [`FetchedArtifact`] is consumed only by a
    /// share that completes, after which the handle fails closed.
    fn share_content(
        &self,
        content: &ShareContent,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name that could navigate directories is **unrepresentable**: with
    /// [`FileName::parse`] the sole constructor, `open_sink` /
    /// `pick_export_target` / the implementation factories cannot receive path
    /// syntax through the "supposedly safe" type — the same fail-closed
    /// enforcement [`crate::launcher::SafeExternalUrl`] has for schemes.
    #[test]
    fn file_name_rejects_path_syntax_and_unshowable_names() {
        for (bad, why) in [
            ("", InvalidFileName::Empty),
            (".", InvalidFileName::PathComponent),
            ("..", InvalidFileName::PathComponent),
            ("../evil", InvalidFileName::Separator),
            ("..\\evil", InvalidFileName::Separator),
            ("a/b", InvalidFileName::Separator),
            ("/etc/passwd", InvalidFileName::Separator),
            ("C:\\boot.ini", InvalidFileName::Separator),
            ("a\0b", InvalidFileName::Control),
            ("a\nb", InvalidFileName::Control),
        ] {
            assert_eq!(
                FileName::parse(bad).unwrap_err(),
                why,
                "{bad:?} must be rejected as {why:?}"
            );
        }
    }

    /// Only the *whole* name `.`/`..` navigates; dots elsewhere are ordinary
    /// characters, and validation normalizes nothing.
    #[test]
    fn file_name_accepts_ordinary_display_names() {
        for good in ["report.pdf", "a..b", "résumé (final).txt", "..hidden.."] {
            assert!(FileName::parse(good).is_ok(), "{good:?} must parse");
        }
    }

    /// §K1: the error renders a discriminant only — a rejected peer-supplied
    /// name never rides the error into a log.
    #[test]
    fn invalid_file_name_carries_no_payload() {
        let rendered = format!("{}", InvalidFileName::Separator);
        assert_eq!(rendered, "file name contains a path separator");
        assert!(
            !rendered.contains('/') && !rendered.contains('\\'),
            "the error must not echo the rejected name: {rendered}"
        );
    }
}
