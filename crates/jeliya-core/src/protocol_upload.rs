//! Opaque, bounded staging for protocol-v2 consumer-direction uploads.
//!
//! The wire runtime owns framing, credit, and terminal races. This module owns
//! the filesystem boundary below it: an admitted share gets one unpredictable
//! exclusive object in a protocol-only staging directory, accepts only
//! contiguous complete records, and turns an exact END into one consuming
//! import/event capability. No path, file, seek, or random-access handle is
//! exposed through the public surface.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use jeliya_api::{ApiError, ByteTotal, FileShare, FileShareOut, StreamAbortReason};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::error::{CoreError, CoreResult};
use crate::supervisor::{
    AuthorizedFileShare, FinalizeFileShareError, RoomSupervisor, FILE_UPLOAD_MAX_BYTES,
};

/// Dedicated to protocol streams. The HTTP route stages under `uploads`, and
/// durable room content lives under `blobs`; cleanup never traverses either.
const PROTOCOL_STAGING_DIR: &str = "protocol-v2-stream-staging";
const STAGING_PREFIX: &str = "upload-";
const STAGING_SUFFIX: &str = ".stage";
const STAGING_NONCE_BYTES: usize = 24;
const CREATE_ATTEMPTS: usize = 16;

type BoxWriter = Pin<Box<dyn AsyncWrite + Send>>;
#[cfg(test)]
type BeforeImportHook = Box<dyn FnOnce(&Path) + Send>;

/// A prepared upload whose protocol validation and authorization have already
/// run, but whose staging object does not yet exist.
///
/// The connection runtime calls [`Self::open_sink`] only after it has reserved
/// both transfer limits and started the absolute deadline.
pub struct PreparedFileShare {
    supervisor: Arc<RoomSupervisor>,
    request: FileShare,
    authorized: AuthorizedFileShare,
}

/// An exclusive, forward-only staging sink.
///
/// It has no path, file, seek, clone, or inner-writer accessor. Dropping it
/// before exact END synchronously unlinks its private staging object.
pub struct OpenFileShareSink {
    parts: Option<PreparedParts>,
    storage: Option<StageStorage>,
    accepted_bytes: u64,
    accepted_digest: blake3::Hasher,
}

/// The single-use capability produced by an exact END.
///
/// Consuming [`Self::finalize`] imports the staged bytes and authors one event.
/// Dropping the capability instead authors nothing and removes the stage.
pub struct FileShareFinalizer {
    parts: Option<PreparedParts>,
    sealed: Option<SealedStage>,
    accepted_digest: [u8; 32],
    #[cfg(test)]
    before_import: Option<BeforeImportHook>,
    #[cfg(test)]
    fail_cleanup: bool,
    #[cfg(test)]
    fail_after_publish: bool,
}

struct PreparedParts {
    supervisor: Arc<RoomSupervisor>,
    request: FileShare,
    authorized: AuthorizedFileShare,
}

struct StageStorage {
    path: PathBuf,
    writer: Option<BoxWriter>,
    inspector: Option<tokio::fs::File>,
    identity: FileIdentity,
    cleanup_armed: bool,
    #[cfg(test)]
    fail_sync: bool,
}

struct SealedStage {
    path: PathBuf,
    inspector: Option<tokio::fs::File>,
    identity: FileIdentity,
    cleanup_armed: bool,
}

/// Owns a just-created name until [`StageStorage`] has installed its own
/// unconditional cleanup guard.
struct CreatedPathGuard {
    path: PathBuf,
    armed: bool,
}

#[derive(Clone, Copy)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

/// A staging refusal or local sink failure.
#[derive(Debug)]
pub enum FileShareSinkError {
    /// `offset + payload_len` was not representable.
    Arithmetic,
    /// A DATA record carried no payload (END is the only empty terminator).
    EmptyRecord,
    /// Aggregate policy wins before declaration, continuity, and sink checks.
    FileTooLarge {
        /// Candidate exclusive end offset.
        candidate: u64,
        /// Served aggregate maximum.
        limit: u64,
    },
    /// The candidate is inside aggregate policy but beyond the declaration.
    DeclaredSizeMismatch {
        /// Declared upload count.
        declared: u64,
        /// Candidate or END count observed.
        observed: u64,
    },
    /// The DATA record was not contiguous with durable accepted storage.
    OffsetMismatch {
        /// Next required offset.
        expected: u64,
        /// Offset supplied by the record.
        observed: u64,
    },
    /// The exclusive staging object or its containing directory could not be
    /// created safely.
    Create(io::Error),
    /// A record write failed. `written` reports bytes written from this record
    /// before the sink was poisoned and the entire stage discarded.
    Write {
        /// Bytes physically written before failure; never accepted storage.
        written: u64,
        /// Underlying failure.
        source: io::Error,
    },
    /// A zero-length/short write prevented accepting the complete record.
    PartialWrite {
        /// Full record payload length.
        expected: u64,
        /// Bytes physically written before the sink was discarded.
        written: u64,
    },
    /// Flushing the complete record failed before acceptance.
    Flush(io::Error),
    /// Durably syncing the complete record failed before acceptance.
    Sync(io::Error),
    /// Removing the private staging name failed. Publication must not begin
    /// while the upload can still leave protocol staging residue.
    Cleanup(io::Error),
    /// The daemon-managed stage disappeared before it could be accepted or
    /// finalized.
    StagingDisappeared,
    /// The private path no longer names the exclusive object that was opened.
    StagingReplaced,
    /// The opened object or its directory entry disagreed with the accepted
    /// contiguous count.
    CountDisagreement {
        /// Count the sink had durably accepted.
        expected: u64,
        /// Count reported by the filesystem.
        observed: u64,
    },
    /// The sink was already poisoned by an earlier local failure.
    SinkClosed,
}

impl fmt::Display for FileShareSinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arithmetic => f.write_str("upload offset arithmetic overflowed"),
            Self::EmptyRecord => f.write_str("an upload DATA record must not be empty"),
            Self::FileTooLarge { candidate, limit } => write!(
                f,
                "upload candidate {candidate} exceeds the aggregate limit {limit}"
            ),
            Self::DeclaredSizeMismatch { declared, observed } => write!(
                f,
                "upload count disagrees with its declaration (declared {declared}, observed {observed})"
            ),
            Self::OffsetMismatch { expected, observed } => write!(
                f,
                "upload DATA is not contiguous (expected {expected}, observed {observed})"
            ),
            Self::Create(error) => write!(f, "could not create protocol upload staging: {error}"),
            Self::Write { written, source } => write!(
                f,
                "protocol upload staging write failed after {written} record bytes: {source}"
            ),
            Self::PartialWrite { expected, written } => write!(
                f,
                "protocol upload staging accepted only {written} of {expected} record bytes"
            ),
            Self::Flush(error) => write!(f, "could not flush protocol upload staging: {error}"),
            Self::Sync(error) => write!(f, "could not sync protocol upload staging: {error}"),
            Self::Cleanup(error) => {
                write!(f, "could not remove protocol upload staging: {error}")
            }
            Self::StagingDisappeared => {
                f.write_str("protocol upload staging disappeared")
            }
            Self::StagingReplaced => {
                f.write_str("protocol upload staging was replaced")
            }
            Self::CountDisagreement { expected, observed } => write!(
                f,
                "protocol upload staging count changed (expected {expected}, observed {observed})"
            ),
            Self::SinkClosed => f.write_str("protocol upload staging is already closed"),
        }
    }
}

impl std::error::Error for FileShareSinkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Create(error)
            | Self::Flush(error)
            | Self::Sync(error)
            | Self::Cleanup(error)
            | Self::Write { source: error, .. } => Some(error),
            Self::Arithmetic
            | Self::EmptyRecord
            | Self::FileTooLarge { .. }
            | Self::DeclaredSizeMismatch { .. }
            | Self::OffsetMismatch { .. }
            | Self::PartialWrite { .. }
            | Self::StagingDisappeared
            | Self::StagingReplaced
            | Self::CountDisagreement { .. }
            | Self::SinkClosed => None,
        }
    }
}

/// Run only the post-ledger half of streamed upload preparation.
pub(crate) async fn prepare_file_share_after_gate(
    supervisor: Arc<RoomSupervisor>,
    request: &FileShare,
) -> Result<PreparedFileShare, ApiError> {
    let authorized = crate::typed::authorize_file_share_after_gate(&supervisor, request).await?;
    Ok(PreparedFileShare {
        supervisor,
        request: request.clone(),
        authorized,
    })
}

impl PreparedFileShare {
    /// Create the exclusive staging object. This is intentionally separate
    /// from authorization so transfer reservations can be acquired first.
    pub async fn open_sink(self) -> Result<OpenFileShareSink, FileShareSinkError> {
        let storage = StageStorage::create(self.supervisor.data_dir()).await?;
        Ok(OpenFileShareSink {
            parts: Some(PreparedParts {
                supervisor: self.supervisor,
                request: self.request,
                authorized: self.authorized,
            }),
            storage: Some(storage),
            accepted_bytes: 0,
            accepted_digest: blake3::Hasher::new(),
        })
    }

    /// The declared logical total reserved for this upload.
    #[must_use]
    pub fn declared_bytes(&self) -> u64 {
        self.request.declared_bytes
    }
}

impl OpenFileShareSink {
    /// The declared logical total reserved for this upload.
    #[must_use]
    pub fn declared_bytes(&self) -> u64 {
        self.parts
            .as_ref()
            .map_or(0, |parts| parts.request.declared_bytes)
    }

    /// Bytes durably and contiguously accepted through complete DATA records.
    #[must_use]
    pub const fn accepted_bytes(&self) -> u64 {
        self.accepted_bytes
    }

    /// Atomically accept one complete DATA payload at its exact offset.
    ///
    /// Policy and continuity are checked before the payload is passed to the
    /// writer. Once any local write/flush/sync/count check fails, the sink is
    /// poisoned and all staging is discarded; partial physical writes can
    /// therefore never become accepted bytes or be resumed.
    pub async fn accept(&mut self, offset: u64, payload: &[u8]) -> Result<u64, FileShareSinkError> {
        if payload.is_empty() {
            return Err(FileShareSinkError::EmptyRecord);
        }
        let payload_len =
            u64::try_from(payload.len()).map_err(|_| FileShareSinkError::Arithmetic)?;
        let candidate = offset
            .checked_add(payload_len)
            .ok_or(FileShareSinkError::Arithmetic)?;

        // The record fixes this refusal order. These checks happen before the
        // continuity check and before the writer sees a payload byte.
        if candidate > FILE_UPLOAD_MAX_BYTES {
            return Err(FileShareSinkError::FileTooLarge {
                candidate,
                limit: FILE_UPLOAD_MAX_BYTES,
            });
        }
        let declared = self.declared_bytes();
        if candidate > declared {
            return Err(FileShareSinkError::DeclaredSizeMismatch {
                declared,
                observed: candidate,
            });
        }
        if offset != self.accepted_bytes {
            return Err(FileShareSinkError::OffsetMismatch {
                expected: self.accepted_bytes,
                observed: offset,
            });
        }

        let Some(storage) = self.storage.as_mut() else {
            return Err(FileShareSinkError::SinkClosed);
        };
        if let Err(error) = storage.verify(self.accepted_bytes).await {
            self.discard().await;
            return Err(error);
        }

        let mut written = 0usize;
        while written < payload.len() {
            let write_result = {
                let writer = storage
                    .writer
                    .as_mut()
                    .ok_or(FileShareSinkError::SinkClosed)?;
                writer.write(&payload[written..]).await
            };
            match write_result {
                Ok(0) => {
                    let error = FileShareSinkError::PartialWrite {
                        expected: payload_len,
                        written: u64::try_from(written).unwrap_or(u64::MAX),
                    };
                    self.discard().await;
                    return Err(error);
                }
                Ok(count) => {
                    written = written
                        .checked_add(count)
                        .ok_or(FileShareSinkError::Arithmetic)?;
                }
                Err(source) => {
                    let error = FileShareSinkError::Write {
                        written: u64::try_from(written).unwrap_or(u64::MAX),
                        source,
                    };
                    self.discard().await;
                    return Err(error);
                }
            }
        }

        let flush_result = match storage.writer.as_mut() {
            Some(writer) => writer.flush().await,
            None => Err(io::Error::new(io::ErrorKind::BrokenPipe, "sink closed")),
        };
        if let Err(error) = flush_result {
            self.discard().await;
            return Err(FileShareSinkError::Flush(error));
        }
        let sync_result = storage.sync_data().await;
        if let Err(error) = sync_result {
            self.discard().await;
            return Err(FileShareSinkError::Sync(error));
        }
        if let Err(error) = storage.verify(candidate).await {
            self.discard().await;
            return Err(error);
        }
        // Hash only a complete, durably accepted record. This keeps the
        // digest aligned with the same atomic acceptance boundary used for
        // credit and offsets, without retaining payloads after this call.
        self.accepted_digest.update(payload);
        self.accepted_bytes = candidate;
        Ok(candidate)
    }

    /// Consume exact END and seal the stage into a one-use finalizer.
    pub async fn finish(
        mut self,
        end_offset: u64,
    ) -> Result<FileShareFinalizer, FileShareSinkError> {
        let declared = self.declared_bytes();
        if end_offset != declared || self.accepted_bytes != declared {
            return Err(FileShareSinkError::DeclaredSizeMismatch {
                declared,
                observed: end_offset,
            });
        }
        let Some(mut storage) = self.storage.take() else {
            return Err(FileShareSinkError::SinkClosed);
        };
        if let Err(error) = storage.flush_sync_verify(self.accepted_bytes).await {
            let _ = storage.cleanup().await;
            return Err(error);
        }
        let sealed = storage.seal();
        let parts = self.parts.take().ok_or(FileShareSinkError::SinkClosed)?;
        Ok(FileShareFinalizer {
            parts: Some(parts),
            sealed: Some(sealed),
            accepted_digest: *self.accepted_digest.finalize().as_bytes(),
            #[cfg(test)]
            before_import: None,
            #[cfg(test)]
            fail_cleanup: false,
            #[cfg(test)]
            fail_after_publish: false,
        })
    }

    async fn discard(&mut self) {
        if let Some(storage) = self.storage.take() {
            let _ = storage.cleanup().await;
        }
    }
}

impl Drop for OpenFileShareSink {
    fn drop(&mut self) {
        if let Some(mut storage) = self.storage.take() {
            storage.unlink_now();
        }
    }
}

impl FileShareFinalizer {
    /// Import and author the one prepared share, then remove staging whether
    /// the operation succeeds or fails. A cancellation of this future still
    /// runs the synchronous unlink guard; the connection runtime is expected
    /// to detach it after END so publication itself is not cancelled.
    pub async fn finalize(mut self) -> Result<FileShareOut, ApiError> {
        let PreparedParts {
            supervisor,
            request,
            authorized,
        } = self
            .parts
            .take()
            .expect("file-share finalizer is single-use");
        let mut sealed = self
            .sealed
            .take()
            .expect("file-share finalizer is single-use");
        let terminal = || sink_failed_terminal(request.declared_bytes);

        // Exact END has already won. Every later filesystem/import failure is
        // therefore one terminal sink failure, never a second stream ABORT or
        // an operation error outside file.share's closed taxonomy.
        if let Err(error) = sealed.verify(request.declared_bytes).await {
            let cleanup = sealed.cleanup().await;
            if cleanup.is_err() {
                return Err(terminal());
            }
            return Err(match error {
                FileShareSinkError::CountDisagreement { observed, .. }
                | FileShareSinkError::DeclaredSizeMismatch { observed, .. } => {
                    declared_size_mismatch(request.declared_bytes, observed)
                }
                _ => terminal(),
            });
        }

        #[cfg(test)]
        if let Some(before_import) = self.before_import.take() {
            before_import(&sealed.path);
        }

        // `blob_import` reopens by path. Bind what it actually imported to the
        // complete sequence durably accepted through DATA, so a same-length
        // replacement in that gap cannot authorize different content.
        let imported = match supervisor
            .import_authorized_file_share(authorized, &sealed.path)
            .await
        {
            Ok(imported) => imported,
            Err(error) => {
                let cleanup = sealed.cleanup().await;
                if cleanup.is_err() {
                    return Err(terminal());
                }
                return Err(match error {
                    FinalizeFileShareError::CountDisagreement { observed_bytes } => {
                        declared_size_mismatch(request.declared_bytes, observed_bytes)
                    }
                    FinalizeFileShareError::Core(_) => terminal(),
                });
            }
        };
        if imported.size_bytes() != request.declared_bytes {
            let observed = imported.size_bytes();
            if sealed.cleanup().await.is_err() {
                return Err(terminal());
            }
            return Err(declared_size_mismatch(request.declared_bytes, observed));
        }
        if imported.hash() != self.accepted_digest {
            let _ = sealed.cleanup().await;
            return Err(terminal());
        }

        // No event may be authored until the private stage is gone. A cleanup
        // refusal consumes this finalizer and leaves the imported blob
        // unreferenced; Drop makes one synchronous best-effort retry.
        #[cfg(test)]
        let cleanup_result = if self.fail_cleanup {
            Err(FileShareSinkError::Cleanup(io::Error::other(
                "injected staging cleanup failure",
            )))
        } else {
            sealed.cleanup().await
        };
        #[cfg(not(test))]
        let cleanup_result = sealed.cleanup().await;
        if cleanup_result.is_err() {
            return Err(terminal());
        }

        let shared = match supervisor.publish_imported_file_share(imported).await {
            Ok(shared) => shared,
            Err(_) => return Err(terminal()),
        };
        #[cfg(test)]
        if self.fail_after_publish {
            return Err(terminal());
        }
        crate::typed::project_streamed_file_share(&supervisor, &request, shared)
            .map_err(|_| terminal())
    }
}

impl Drop for FileShareFinalizer {
    fn drop(&mut self) {
        if let Some(mut sealed) = self.sealed.take() {
            sealed.unlink_now();
        }
    }
}

impl StageStorage {
    async fn create(data_dir: &Path) -> Result<Self, FileShareSinkError> {
        let directory = ensure_staging_dir(data_dir).map_err(FileShareSinkError::Create)?;
        for _ in 0..CREATE_ATTEMPTS {
            let mut nonce = [0_u8; STAGING_NONCE_BYTES];
            getrandom::fill(&mut nonce).map_err(|error| {
                FileShareSinkError::Create(io::Error::other(format!(
                    "OS CSPRNG unavailable: {error}"
                )))
            })?;
            let path = directory.join(format!(
                "{STAGING_PREFIX}{}{STAGING_SUFFIX}",
                hex::encode(nonce)
            ));
            // Declared before `file` so error unwinding closes the handle
            // before this guard removes its name (required on Windows).
            let mut created = CreatedPathGuard {
                path: path.clone(),
                armed: false,
            };
            let opened = open_exclusive(&path);
            let file = match opened {
                Ok(file) => {
                    created.armed = true;
                    file
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(FileShareSinkError::Create(error)),
            };
            let identity = FileIdentity::of(&file.metadata().map_err(FileShareSinkError::Create)?);
            let inspector = file.try_clone().map_err(FileShareSinkError::Create)?;
            let storage = Self {
                path,
                writer: Some(Box::pin(tokio::fs::File::from_std(file))),
                inspector: Some(tokio::fs::File::from_std(inspector)),
                identity,
                cleanup_armed: true,
                #[cfg(test)]
                fail_sync: false,
            };
            created.armed = false;
            return Ok(storage);
        }
        Err(FileShareSinkError::Create(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique protocol staging object",
        )))
    }

    async fn verify(&mut self, expected: u64) -> Result<(), FileShareSinkError> {
        let inspector = self
            .inspector
            .as_ref()
            .ok_or(FileShareSinkError::SinkClosed)?;
        let opened = inspector
            .metadata()
            .await
            .map_err(|error| classify_metadata_error(&error))?;
        let named = tokio::fs::symlink_metadata(&self.path)
            .await
            .map_err(|error| classify_metadata_error(&error))?;
        if !opened.is_file() || !named.is_file() || named.file_type().is_symlink() {
            return Err(FileShareSinkError::StagingReplaced);
        }
        if !self.identity.matches(&opened) || !self.identity.matches(&named) {
            return Err(FileShareSinkError::StagingReplaced);
        }
        if opened.len() != expected {
            return Err(FileShareSinkError::CountDisagreement {
                expected,
                observed: opened.len(),
            });
        }
        if named.len() != expected {
            return Err(FileShareSinkError::CountDisagreement {
                expected,
                observed: named.len(),
            });
        }
        Ok(())
    }

    async fn flush_sync_verify(&mut self, expected: u64) -> Result<(), FileShareSinkError> {
        let writer = self.writer.as_mut().ok_or(FileShareSinkError::SinkClosed)?;
        writer.flush().await.map_err(FileShareSinkError::Flush)?;
        self.sync_data().await.map_err(FileShareSinkError::Sync)?;
        self.verify(expected).await
    }

    async fn sync_data(&mut self) -> io::Result<()> {
        #[cfg(test)]
        if self.fail_sync {
            return Err(io::Error::other("injected sync failure"));
        }
        match self.inspector.as_ref() {
            Some(inspector) => inspector.sync_data().await,
            None => Err(io::Error::new(io::ErrorKind::BrokenPipe, "sink closed")),
        }
    }

    fn seal(mut self) -> SealedStage {
        self.writer.take();
        let sealed = SealedStage {
            path: self.path.clone(),
            inspector: self.inspector.take(),
            identity: self.identity,
            cleanup_armed: true,
        };
        self.cleanup_armed = false;
        sealed
    }

    async fn cleanup(mut self) -> Result<(), FileShareSinkError> {
        self.writer.take();
        self.inspector.take();
        match tokio::fs::remove_file(&self.path).await {
            Ok(()) => {
                self.cleanup_armed = false;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.cleanup_armed = false;
                Ok(())
            }
            Err(error) => Err(FileShareSinkError::Cleanup(error)),
        }
    }

    fn unlink_now(&mut self) {
        self.writer.take();
        self.inspector.take();
        match std::fs::remove_file(&self.path) {
            Ok(()) => self.cleanup_armed = false,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.cleanup_armed = false;
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "could not remove dropped protocol upload staging residue"
                );
            }
        }
    }
}

impl SealedStage {
    async fn verify(&mut self, expected: u64) -> Result<(), FileShareSinkError> {
        let inspector = self
            .inspector
            .as_ref()
            .ok_or(FileShareSinkError::SinkClosed)?;
        let opened = inspector
            .metadata()
            .await
            .map_err(|error| classify_metadata_error(&error))?;
        let named = tokio::fs::symlink_metadata(&self.path)
            .await
            .map_err(|error| classify_metadata_error(&error))?;
        if !opened.is_file()
            || !named.is_file()
            || named.file_type().is_symlink()
            || !self.identity.matches(&opened)
            || !self.identity.matches(&named)
        {
            return Err(FileShareSinkError::StagingReplaced);
        }
        if opened.len() != expected {
            return Err(FileShareSinkError::CountDisagreement {
                expected,
                observed: opened.len(),
            });
        }
        if named.len() != expected {
            return Err(FileShareSinkError::CountDisagreement {
                expected,
                observed: named.len(),
            });
        }
        Ok(())
    }

    async fn cleanup(mut self) -> Result<(), FileShareSinkError> {
        self.inspector.take();
        match tokio::fs::remove_file(&self.path).await {
            Ok(()) => {
                self.cleanup_armed = false;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.cleanup_armed = false;
                Ok(())
            }
            Err(error) => Err(FileShareSinkError::Cleanup(error)),
        }
    }

    fn unlink_now(&mut self) {
        self.inspector.take();
        match std::fs::remove_file(&self.path) {
            Ok(()) => self.cleanup_armed = false,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.cleanup_armed = false;
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "could not remove dropped protocol upload finalizer residue"
                );
            }
        }
    }
}

impl Drop for CreatedPathGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl Drop for StageStorage {
    fn drop(&mut self) {
        if self.cleanup_armed {
            self.unlink_now();
        }
    }
}

impl Drop for SealedStage {
    fn drop(&mut self) {
        if self.cleanup_armed {
            self.unlink_now();
        }
    }
}

impl FileIdentity {
    fn of(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            Self {}
        }
    }

    fn matches(&self, metadata: &std::fs::Metadata) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.device == metadata.dev() && self.inode == metadata.ino()
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            true
        }
    }
}

fn classify_metadata_error(error: &io::Error) -> FileShareSinkError {
    if error.kind() == io::ErrorKind::NotFound {
        FileShareSinkError::StagingDisappeared
    } else {
        FileShareSinkError::Create(io::Error::new(error.kind(), error.to_string()))
    }
}

fn sink_failed_terminal(declared_bytes: u64) -> ApiError {
    ApiError::StreamAborted {
        transferred_bytes: declared_bytes,
        total: ByteTotal::Known {
            bytes: declared_bytes,
        },
        reason: StreamAbortReason::SinkFailed,
    }
}

fn declared_size_mismatch(declared_bytes: u64, observed_bytes: u64) -> ApiError {
    ApiError::DeclaredSizeMismatch {
        declared_bytes,
        observed_bytes,
    }
}

fn ensure_staging_dir(data_dir: &Path) -> io::Result<PathBuf> {
    let root = std::fs::canonicalize(data_dir)?;
    let directory = root.join(PROTOCOL_STAGING_DIR);
    match std::fs::symlink_metadata(&directory) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "protocol staging root is not a private directory",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match std::fs::create_dir(&directory) {
                Ok(()) => {}
                Err(raced) if raced.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(&directory)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "protocol staging root changed type",
        ));
    }
    let resolved = std::fs::canonicalize(&directory)?;
    if resolved.parent() != Some(root.as_path()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "protocol staging root escaped the daemon data directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&resolved, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(resolved)
}

fn open_exclusive(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn is_protocol_stage_name(name: &str) -> bool {
    let Some(hex) = name
        .strip_prefix(STAGING_PREFIX)
        .and_then(|name| name.strip_suffix(STAGING_SUFFIX))
    else {
        return false;
    };
    hex.len() == STAGING_NONCE_BYTES * 2 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Remove abandoned protocol-stream stages at daemon startup.
///
/// The walk is deliberately one directory deep and recognizes only this
/// module's unpredictable stage-name shape. It cannot enter the HTTP upload
/// directory or any durable blob tree.
pub(crate) fn cleanup_abandoned_protocol_uploads(data_dir: &Path) -> CoreResult<usize> {
    let root = std::fs::canonicalize(data_dir).map_err(|error| {
        CoreError::internal(format!("could not resolve the daemon data dir: {error}"))
    })?;
    let directory = root.join(PROTOCOL_STAGING_DIR);
    let metadata = match std::fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(CoreError::internal(format!(
                "could not inspect protocol staging at startup: {error}"
            )))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CoreError::internal(
            "protocol staging root is not a private directory",
        ));
    }
    let resolved = std::fs::canonicalize(&directory).map_err(|error| {
        CoreError::internal(format!("could not resolve protocol staging: {error}"))
    })?;
    if resolved.parent() != Some(root.as_path()) {
        return Err(CoreError::internal(
            "protocol staging root escaped the daemon data directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&resolved, std::fs::Permissions::from_mode(0o700)).map_err(
            |error| {
                CoreError::internal(format!(
                    "could not make protocol staging private at startup: {error}"
                ))
            },
        )?;
    }

    let entries = std::fs::read_dir(&resolved).map_err(|error| {
        CoreError::internal(format!("could not scan protocol staging: {error}"))
    })?;
    let mut removed = 0usize;
    for entry in entries {
        let entry = entry.map_err(|error| {
            CoreError::internal(format!("could not read protocol staging entry: {error}"))
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !is_protocol_stage_name(&name) {
            continue;
        }
        let entry_type = entry.file_type().map_err(|error| {
            CoreError::internal(format!("could not inspect protocol staging entry: {error}"))
        })?;
        if entry_type.is_dir() {
            return Err(CoreError::internal(
                "a protocol staging artifact unexpectedly became a directory",
            ));
        }
        std::fs::remove_file(entry.path()).map_err(|error| {
            CoreError::internal(format!(
                "could not remove abandoned protocol stage: {error}"
            ))
        })?;
        removed = removed.saturating_add(1);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::{Context, Poll};

    use iroh_rooms::events::EventType;
    use jeliya_api::RoomId as ApiRoomId;
    use tempfile::tempdir;

    struct ScriptedWriter {
        first: usize,
        wrote: bool,
        flush_fails: bool,
    }

    impl AsyncWrite for ScriptedWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            if self.wrote {
                return Poll::Ready(Err(io::Error::other("injected write failure")));
            }
            self.wrote = true;
            Poll::Ready(Ok(self.first.min(bytes.len())))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            if self.flush_fails {
                Poll::Ready(Err(io::Error::other("injected flush failure")))
            } else {
                Poll::Ready(Ok(()))
            }
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    async fn setup_room(data_dir: &Path) -> (Arc<RoomSupervisor>, String) {
        crate::identity::create(data_dir).unwrap();
        let supervisor = Arc::new(RoomSupervisor::new(data_dir.to_path_buf(), true).unwrap());
        let room_id = supervisor.create_room("stream staging").unwrap();
        supervisor.open_room(&room_id, &[]).await.unwrap();
        (supervisor, room_id)
    }

    fn request(room_id: &str, declared_bytes: u64) -> FileShare {
        FileShare {
            room_id: ApiRoomId::new(room_id),
            name: "payload.bin".into(),
            declared_bytes,
            declared_content_type: "application/octet-stream".into(),
        }
    }

    async fn open_sink(
        supervisor: &Arc<RoomSupervisor>,
        room_id: &str,
        declared_bytes: u64,
    ) -> OpenFileShareSink {
        prepare_file_share_after_gate(supervisor.clone(), &request(room_id, declared_bytes))
            .await
            .unwrap()
            .open_sink()
            .await
            .unwrap()
    }

    fn stage_path(sink: &OpenFileShareSink) -> PathBuf {
        sink.storage.as_ref().unwrap().path.clone()
    }

    fn file_event_count(supervisor: &RoomSupervisor, room_id: &str) -> usize {
        let room_id = room_id.parse().unwrap();
        supervisor
            .open_store()
            .unwrap()
            .by_type(&room_id, EventType::FileShared)
            .unwrap()
            .len()
    }

    #[test]
    fn startup_cleanup_is_confined_to_protocol_artifacts() {
        let dir = tempdir().unwrap();
        crate::identity::ensure_dir(dir.path()).unwrap();
        let protocol = dir.path().join(PROTOCOL_STAGING_DIR);
        let http = dir.path().join("uploads");
        let blobs = dir.path().join("blobs");
        std::fs::create_dir_all(&protocol).unwrap();
        std::fs::create_dir_all(&http).unwrap();
        std::fs::create_dir_all(&blobs).unwrap();
        let abandoned = protocol.join(format!(
            "{STAGING_PREFIX}{}{STAGING_SUFFIX}",
            "ab".repeat(STAGING_NONCE_BYTES)
        ));
        let unrelated = protocol.join("operator-note");
        let http_stage = http.join("upload.bin");
        let durable = blobs.join("durable.bin");
        std::fs::write(&abandoned, b"partial").unwrap();
        std::fs::write(&unrelated, b"keep").unwrap();
        std::fs::write(&http_stage, b"http").unwrap();
        std::fs::write(&durable, b"blob").unwrap();

        assert_eq!(cleanup_abandoned_protocol_uploads(dir.path()).unwrap(), 1);
        assert!(!abandoned.exists());
        assert!(unrelated.exists());
        assert!(http_stage.exists());
        assert!(durable.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&protocol).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn startup_cleanup_repairs_existing_staging_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        crate::identity::ensure_dir(dir.path()).unwrap();
        let protocol = dir.path().join(PROTOCOL_STAGING_DIR);
        std::fs::create_dir(&protocol).unwrap();
        std::fs::set_permissions(&protocol, std::fs::Permissions::from_mode(0o777)).unwrap();

        assert_eq!(cleanup_abandoned_protocol_uploads(dir.path()).unwrap(), 0);
        assert_eq!(
            std::fs::metadata(protocol).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[tokio::test]
    async fn exclusive_stage_is_unlinked_on_drop_and_disappearance_is_detected() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;
        let sink = open_sink(&supervisor, &room_id, 1).await;
        let guarded_path = stage_path(&sink);
        let other = open_sink(&supervisor, &room_id, 1).await;
        let other_path = stage_path(&other);
        assert!(guarded_path.exists());
        assert_ne!(guarded_path, other_path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&guarded_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        drop(other);
        assert!(!other_path.exists());
        drop(sink);
        assert!(!guarded_path.exists());

        let mut sink = open_sink(&supervisor, &room_id, 1).await;
        let path = stage_path(&sink);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            sink.accept(0, b"x").await,
            Err(FileShareSinkError::StagingDisappeared)
        ));
        assert_eq!(sink.accepted_bytes(), 0);
        assert!(!path.exists());
        drop(sink);
        supervisor.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn preparation_and_declared_policy_create_no_staging_object() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;
        let staging = dir.path().join(PROTOCOL_STAGING_DIR);

        let prepared = prepare_file_share_after_gate(
            supervisor.clone(),
            &request(&room_id, FILE_UPLOAD_MAX_BYTES),
        )
        .await
        .unwrap();
        assert!(!staging.exists());
        drop(prepared);

        let refused = prepare_file_share_after_gate(
            supervisor.clone(),
            &request(&room_id, FILE_UPLOAD_MAX_BYTES + 1),
        )
        .await;
        assert!(matches!(
            refused,
            Err(ApiError::FileTooLarge {
                declared_bytes,
                limit_bytes: FILE_UPLOAD_MAX_BYTES,
                enforced_at: jeliya_api::EnforcedAt::StageDeclared,
            }) if declared_bytes == FILE_UPLOAD_MAX_BYTES + 1
        ));
        assert!(!staging.exists());
        assert_eq!(file_event_count(&supervisor, &room_id), 0);
        supervisor.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn partial_write_and_flush_failure_are_detected_before_acceptance() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;

        let mut partial = open_sink(&supervisor, &room_id, 4).await;
        let partial_path = stage_path(&partial);
        partial.storage.as_mut().unwrap().writer = Some(Box::pin(ScriptedWriter {
            first: 2,
            wrote: false,
            flush_fails: false,
        }));
        let error = partial.accept(0, b"abcd").await.unwrap_err();
        assert!(matches!(
            error,
            FileShareSinkError::Write { written: 2, .. }
        ));
        assert_eq!(partial.accepted_bytes(), 0);
        assert!(!partial_path.exists());

        let mut zero_write = open_sink(&supervisor, &room_id, 4).await;
        let zero_path = stage_path(&zero_write);
        zero_write.storage.as_mut().unwrap().writer = Some(Box::pin(ScriptedWriter {
            first: 0,
            wrote: false,
            flush_fails: false,
        }));
        assert!(matches!(
            zero_write.accept(0, b"abcd").await,
            Err(FileShareSinkError::PartialWrite {
                expected: 4,
                written: 0
            })
        ));
        assert_eq!(zero_write.accepted_bytes(), 0);
        assert!(!zero_path.exists());

        let mut flush = open_sink(&supervisor, &room_id, 4).await;
        let flush_path = stage_path(&flush);
        flush.storage.as_mut().unwrap().writer = Some(Box::pin(ScriptedWriter {
            first: usize::MAX,
            wrote: false,
            flush_fails: true,
        }));
        assert!(matches!(
            flush.accept(0, b"abcd").await,
            Err(FileShareSinkError::Flush(_))
        ));
        assert_eq!(flush.accepted_bytes(), 0);
        assert!(!flush_path.exists());

        let mut sync = open_sink(&supervisor, &room_id, 4).await;
        let sync_path = stage_path(&sync);
        sync.storage.as_mut().unwrap().fail_sync = true;
        assert!(matches!(
            sync.accept(0, b"abcd").await,
            Err(FileShareSinkError::Sync(_))
        ));
        assert_eq!(sync.accepted_bytes(), 0);
        assert!(!sync_path.exists());

        let mut finish_sync = open_sink(&supervisor, &room_id, 1).await;
        finish_sync.accept(0, b"x").await.unwrap();
        let finish_sync_path = stage_path(&finish_sync);
        finish_sync.storage.as_mut().unwrap().fail_sync = true;
        assert!(matches!(
            finish_sync.finish(1).await,
            Err(FileShareSinkError::Sync(_))
        ));
        assert!(!finish_sync_path.exists());
        assert_eq!(file_event_count(&supervisor, &room_id), 0);
        drop((partial, zero_write, flush, sync));
        supervisor.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn policy_precedes_declaration_and_continuity_before_any_copy() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;

        let mut aggregate = open_sink(&supervisor, &room_id, FILE_UPLOAD_MAX_BYTES).await;
        let aggregate_path = stage_path(&aggregate);
        assert!(matches!(
            aggregate.accept(FILE_UPLOAD_MAX_BYTES, b"x").await,
            Err(FileShareSinkError::FileTooLarge {
                candidate,
                limit: FILE_UPLOAD_MAX_BYTES
            }) if candidate == FILE_UPLOAD_MAX_BYTES + 1
        ));
        assert_eq!(std::fs::metadata(&aggregate_path).unwrap().len(), 0);

        let mut declaration = open_sink(&supervisor, &room_id, 1).await;
        let declaration_path = stage_path(&declaration);
        assert!(matches!(
            declaration.accept(1, b"x").await,
            Err(FileShareSinkError::DeclaredSizeMismatch {
                declared: 1,
                observed: 2
            })
        ));
        assert_eq!(std::fs::metadata(&declaration_path).unwrap().len(), 0);

        let mut arithmetic = open_sink(&supervisor, &room_id, 1).await;
        let arithmetic_path = stage_path(&arithmetic);
        assert!(matches!(
            arithmetic.accept(u64::MAX, b"x").await,
            Err(FileShareSinkError::Arithmetic)
        ));
        assert_eq!(std::fs::metadata(&arithmetic_path).unwrap().len(), 0);

        let mut discontinuous = open_sink(&supervisor, &room_id, 2).await;
        let discontinuous_path = stage_path(&discontinuous);
        assert!(matches!(
            discontinuous.accept(1, b"x").await,
            Err(FileShareSinkError::OffsetMismatch {
                expected: 0,
                observed: 1
            })
        ));
        assert_eq!(std::fs::metadata(&discontinuous_path).unwrap().len(), 0);
        drop((aggregate, declaration, arithmetic, discontinuous));
        assert!(!aggregate_path.exists());
        assert!(!declaration_path.exists());
        assert!(!arithmetic_path.exists());
        assert!(!discontinuous_path.exists());
        supervisor.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn records_are_contiguous_durable_and_end_seals_exactly() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;
        let mut sink = open_sink(&supervisor, &room_id, 5).await;
        let path = stage_path(&sink);

        assert_eq!(sink.accept(0, b"ab").await.unwrap(), 2);
        assert_eq!(sink.accepted_bytes(), 2);
        assert_eq!(std::fs::read(&path).unwrap(), b"ab");
        assert!(matches!(
            sink.accept(1, b"x").await,
            Err(FileShareSinkError::OffsetMismatch {
                expected: 2,
                observed: 1
            })
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"ab");
        assert_eq!(sink.accept(2, b"cde").await.unwrap(), 5);
        assert_eq!(std::fs::read(&path).unwrap(), b"abcde");
        assert_eq!(file_event_count(&supervisor, &room_id), 0);

        let finalizer = sink.finish(5).await.unwrap();
        assert!(path.exists());
        assert_eq!(file_event_count(&supervisor, &room_id), 0);
        let output = finalizer.finalize().await.unwrap();
        assert_eq!(output.bytes, 5);
        assert_eq!(
            output.digest,
            iroh_rooms::files::HashRef::from_bytes(*blake3::hash(b"abcde").as_bytes()).to_string()
        );
        assert_eq!(file_event_count(&supervisor, &room_id), 1);
        assert!(!path.exists());
        supervisor.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn zero_byte_finalize_and_below_end_cleanup() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;

        let zero = open_sink(&supervisor, &room_id, 0).await;
        let zero_path = stage_path(&zero);
        let output = zero.finish(0).await.unwrap().finalize().await.unwrap();
        assert_eq!(output.bytes, 0);
        assert!(!zero_path.exists());

        let mut below = open_sink(&supervisor, &room_id, 2).await;
        let below_path = stage_path(&below);
        below.accept(0, b"x").await.unwrap();
        assert!(matches!(
            below.finish(1).await,
            Err(FileShareSinkError::DeclaredSizeMismatch {
                declared: 2,
                observed: 1
            })
        ));
        assert!(!below_path.exists());
        assert_eq!(file_event_count(&supervisor, &room_id), 1);
        supervisor.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn count_replacement_and_post_end_disappearance_never_author() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;

        let mut count = open_sink(&supervisor, &room_id, 1).await;
        let count_path = stage_path(&count);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&count_path)
            .unwrap()
            .set_len(1)
            .unwrap();
        assert!(matches!(
            count.accept(0, b"x").await,
            Err(FileShareSinkError::CountDisagreement {
                expected: 0,
                observed: 1
            })
        ));
        assert!(!count_path.exists());

        let mut replaced = open_sink(&supervisor, &room_id, 1).await;
        let replaced_path = stage_path(&replaced);
        std::fs::remove_file(&replaced_path).unwrap();
        std::fs::write(&replaced_path, b"").unwrap();
        assert!(matches!(
            replaced.accept(0, b"x").await,
            Err(FileShareSinkError::StagingReplaced)
        ));
        assert!(!replaced_path.exists());

        let mut finalizing = open_sink(&supervisor, &room_id, 1).await;
        finalizing.accept(0, b"x").await.unwrap();
        let finalizer = finalizing.finish(1).await.unwrap();
        let final_path = finalizer.sealed.as_ref().unwrap().path.clone();
        std::fs::remove_file(&final_path).unwrap();
        assert_eq!(
            finalizer.finalize().await,
            Err(ApiError::StreamAborted {
                transferred_bytes: 1,
                total: ByteTotal::Known { bytes: 1 },
                reason: StreamAbortReason::SinkFailed,
            })
        );
        assert!(!final_path.exists());
        assert_eq!(file_event_count(&supervisor, &room_id), 0);
        drop((count, replaced));
        supervisor.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn import_is_bound_to_the_incrementally_accepted_digest() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;
        let mut sink = open_sink(&supervisor, &room_id, 4).await;
        sink.accept(0, b"good").await.unwrap();
        let mut finalizer = sink.finish(4).await.unwrap();
        let path = finalizer.sealed.as_ref().unwrap().path.clone();
        finalizer.before_import = Some(Box::new(|path| {
            std::fs::remove_file(path).unwrap();
            std::fs::write(path, b"evil").unwrap();
        }));

        assert_eq!(finalizer.finalize().await, Err(sink_failed_terminal(4)));
        assert!(!path.exists());
        assert_eq!(file_event_count(&supervisor, &room_id), 0);
        supervisor.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn finalization_requires_cleanup_before_publication() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;
        let mut sink = open_sink(&supervisor, &room_id, 1).await;
        sink.accept(0, b"x").await.unwrap();
        let mut finalizer = sink.finish(1).await.unwrap();
        let path = finalizer.sealed.as_ref().unwrap().path.clone();
        finalizer.fail_cleanup = true;

        assert_eq!(finalizer.finalize().await, Err(sink_failed_terminal(1)));
        assert!(!path.exists());
        assert_eq!(file_event_count(&supervisor, &room_id), 0);
        supervisor.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn post_end_count_change_and_dropped_finalizer_leave_no_event_or_stage() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;

        let mut changed = open_sink(&supervisor, &room_id, 1).await;
        changed.accept(0, b"x").await.unwrap();
        let changed_finalizer = changed.finish(1).await.unwrap();
        let changed_path = changed_finalizer.sealed.as_ref().unwrap().path.clone();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&changed_path)
            .unwrap()
            .set_len(2)
            .unwrap();
        assert_eq!(
            changed_finalizer.finalize().await,
            Err(declared_size_mismatch(1, 2))
        );
        assert!(!changed_path.exists());

        let mut dropped = open_sink(&supervisor, &room_id, 1).await;
        dropped.accept(0, b"y").await.unwrap();
        let dropped_finalizer = dropped.finish(1).await.unwrap();
        let dropped_path = dropped_finalizer.sealed.as_ref().unwrap().path.clone();
        drop(dropped_finalizer);
        assert!(!dropped_path.exists());
        assert_eq!(file_event_count(&supervisor, &room_id), 0);
        supervisor.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn post_publication_projection_failure_uses_legal_terminal_once() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;
        let mut sink = open_sink(&supervisor, &room_id, 1).await;
        sink.accept(0, b"x").await.unwrap();
        let mut finalizer = sink.finish(1).await.unwrap();
        let path = finalizer.sealed.as_ref().unwrap().path.clone();
        finalizer.fail_after_publish = true;

        assert_eq!(finalizer.finalize().await, Err(sink_failed_terminal(1)));
        assert!(!path.exists());
        assert_eq!(file_event_count(&supervisor, &room_id), 1);
        supervisor.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn host_staged_api_still_preserves_its_path_and_typed_result() {
        let dir = tempdir().unwrap();
        let (supervisor, room_id) = setup_room(dir.path()).await;
        let path = dir.path().join("host-upload.bin");
        std::fs::write(&path, b"host").unwrap();
        let req = request(&room_id, 4);

        let output = crate::typed::TypedSupervisor::new(&supervisor)
            .share_staged_file(&req, &path)
            .await
            .unwrap();
        assert_eq!(output.bytes, 4);
        assert!(path.exists(), "the host owns and retains its staged path");
        assert_eq!(file_event_count(&supervisor, &room_id), 1);

        let mismatch = request(&room_id, 3);
        assert_eq!(
            crate::typed::TypedSupervisor::new(&supervisor)
                .share_staged_file(&mismatch, &path)
                .await,
            Err(declared_size_mismatch(3, 4))
        );
        assert!(path.exists());
        assert_eq!(file_event_count(&supervisor, &room_id), 1);
        supervisor.close_room(&room_id).await.unwrap();
    }
}
