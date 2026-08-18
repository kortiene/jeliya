//! The upload flow (spec §5.B): pick → bounded staged copy → `file.share`.
//!
//! State: `Idle → Picking → Staging(progress) → Uploading(progress) →
//! Settled(Shared | Failed | Cancelled)`. Every over-limit / empty / unreadable
//! / cancel path maps to a typed outcome; the daemon's `FileShareOut` is the
//! only success proof (no fabricated "shared"). Preflight refuses a known
//! -oversize source **before any copy** (spec D3); a cancel reaches both the
//! local copy and the wire, and cleans up the staged blob (spec D5).
//!
//! The seam's byte-stream depth is deferred (#269/#171): `call_stream` resolves
//! to `FileShareOut` today, and the staged pull-reader is opened to prove the
//! plumbing and held across the stream. The orchestration and outcome
//! classification are complete and transport-agnostic; only the concrete byte
//! pump arrives with the real transport.

use jeliya_api::{FileId, FileShare, RoomId};
use jeliya_client::{ClientHandle, Dedup};
use jeliya_platform::{CancelToken, PlatformServices, ProgressSink};

use super::errors::{FlowFailure, FlowStop, Settled};
use super::size::{self, ServedLimit};
use super::transfer::race_cancel_stream;

/// The success payload of an upload: the daemon's shared-file identity.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UploadDone {
    /// The shared file's id.
    pub file_id: FileId,
    /// The accepted byte count.
    pub bytes: u64,
}

/// The coarse phase of an in-flight upload, for the UI's progress affordance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UploadPhase {
    /// Waiting on the platform picker.
    Picking,
    /// Copying bytes into staging custody.
    Staging,
    /// Streaming to the daemon.
    Uploading,
}

/// Run one upload to settlement. Drives the `PlatformServices` picker/stager and
/// the `ClientHandle` `file.share` stream, racing an explicit cancel.
///
/// `limit` is the **served** ceiling (spec D3): a known-oversize source is
/// refused before any copy; when the limit is absent (seam gap, spec R1) the
/// staging copy is bounded to `u64::MAX` so no compiled-in ceiling is invented,
/// and the daemon re-enforces.
pub async fn run_upload(
    handle: &ClientHandle,
    services: &PlatformServices,
    room_id: RoomId,
    limit: ServedLimit,
    progress: ProgressSink,
    ct: &CancelToken,
) -> Settled<UploadDone> {
    let files = services.files();

    // 1. Pick. A clean no-selection is "nothing to share" (Cancelled-shaped: the
    //    prior state is untouched); a user dismissal is a distinct Cancelled.
    let src = match files.pick(ct).await {
        Ok(Some(src)) => src,
        Ok(None) => return Settled::Cancelled,
        Err(error) => return Settled::from_stop(FlowStop::from_capability(error)),
    };

    // 2. Preflight the known size BEFORE any copy (spec D3). On refusal, release
    //    the un-staged pick and surface the served integers.
    if let Err(over) = size::preflight(limit, src.size()) {
        let _ = files.discard_source(&src).await;
        return Settled::Failed(FlowFailure::TooLarge {
            size: over.size,
            limit: over.limit,
        });
    }

    // The staging ceiling: the served value, or `u64::MAX` when absent — NEVER a
    // compiled-in constant (spec D3/R1). The daemon re-enforces regardless.
    let stage_limit = limit.map(size::SizeLimit::max_bytes).unwrap_or(u64::MAX);

    // 3. Stage: a bounded, size-enforced, cancel-cleans-up copy into custody. A
    //    staged pick is consumed by staging, so it needs no later discard.
    let blob = match files.stage_for_share(&src, stage_limit, progress, ct).await {
        Ok(blob) => blob,
        Err(error) => return Settled::from_stop(FlowStop::from_capability(error)),
    };

    // 4. Open the pull-reader the `file.share` upload draws from under CREDIT.
    //    (Depth deferred: the seam does not yet pull it — #269/#171.) On failure,
    //    release the staged bytes so nothing lingers in custody.
    let _reader = match files.read_staged(&blob).await {
        Ok(reader) => reader,
        Err(error) => {
            let _ = files.release_staged(&blob).await;
            return Settled::from_stop(FlowStop::from_capability(error));
        }
    };

    // 5. Issue `file.share` as a streaming call, racing an explicit cancel. The
    //    declared type is the picked source's MIME (untrusted, echoed as declared).
    let request = FileShare {
        room_id,
        name: src.display_name().as_str().to_string(),
        declared_bytes: blob.size(),
        declared_content_type: src
            .mime()
            .map(|mime| mime.as_str().to_string())
            .unwrap_or_default(),
    };
    let mut stream = handle.call_stream::<FileShare>(request, Dedup::None);
    let outcome = race_cancel_stream(&mut stream, ct).await;

    // 6. Release the staged blob once the operation SETTLED (success OR failure),
    //    the outbound half of "delete after share". A cancel already aborted the
    //    stream; releasing here leaks no bytes into the data dir either way.
    let _ = files.release_staged(&blob).await;

    match outcome {
        Ok(out) => Settled::Ok(UploadDone {
            file_id: out.file_id,
            bytes: out.bytes,
        }),
        Err(stop) => Settled::from_stop(stop),
    }
}
