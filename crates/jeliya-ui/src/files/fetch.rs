//! The fetch → read → export flow (spec §5.B).
//!
//! State: `Idle → Fetching(progress) → Reading(progress) → Exporting →
//! Settled`. The signed size is preflighted against the served limit **before**
//! contacting a provider (spec D3), so a size refusal is caught early and is
//! *never* rendered as a digest/integrity failure. `ProviderUnreachable` renders
//! the attempted-provider evidence (from the `file.list` model, spec D2);
//! `DigestMismatch` is a distinct integrity outcome.
//!
//! Cancellation is honest and reaches the right mechanism (spec D5): the
//! `file.fetch` phase is op_id-addressed, so a cancel there issues
//! `transfer.cancel { transfer_op_id }` (survives reconnect); the `file.read`
//! phase cancels the stream in-band; and the export sink is dropped uncommitted
//! (drop-is-abort deletes any partial artifact). The export destination is an
//! opaque `ExportTarget` the browser owns — no write path is ever named (spec
//! D4).
//!
//! Byte-stream depth is deferred (#269/#171): `call_stream` resolves to
//! `FileReadOut` today and the sink is opened + committed to prove the plumbing;
//! the concrete DATA pump arrives with the real transport.

use jeliya_api::{FileFetch, FileId, FileRead, OpId, RoomId, TransferCancel};
use jeliya_client::{ClientHandle, Dedup};
use jeliya_platform::{CancelToken, FileName, PlatformServices};

use super::errors::{FlowStop, Settled};
use super::size::{self, ServedLimit};
use super::transfer::{race_cancel_call, race_cancel_stream};

/// The op_id a fetch is issued under, so a later `transfer.cancel` (or a
/// cross-reconnect replay) can name it. Deterministic per `(room, file)`: stable
/// enough to address the transfer, and it carries no secret.
fn fetch_op_id(room_id: &RoomId, file_id: &FileId) -> OpId {
    OpId::new(format!(
        "file.fetch:{}:{}",
        room_id.as_str(),
        file_id.as_str()
    ))
}

/// Run one fetch → read → **export/download** to settlement.
///
/// `signed_bytes` is the file's signed size from its `file.list` row,
/// preflighted against the served limit before any provider is contacted (spec
/// D3). `suggested` is the validated export name (a hostile peer name never
/// reaches here — the caller passes the row's parsed [`FileName`], suppressing
/// export when the name failed validation).
// The parameters are the two injected seams plus the file identity, its signed
// size, the validated export name, the served limit, and the cancel token —
// each a distinct, meaningful input a fetch cannot omit. Bundling them into a
// params struct would only move the same fields behind one name.
#[allow(clippy::too_many_arguments)]
pub async fn run_fetch_export(
    handle: &ClientHandle,
    services: &PlatformServices,
    room_id: RoomId,
    file_id: FileId,
    signed_bytes: u64,
    suggested: FileName,
    limit: ServedLimit,
    ct: &CancelToken,
) -> Settled<()> {
    let files = services.files();

    // 1. Preflight the SIGNED size against the served limit (spec D3): a size
    //    refusal is caught before contacting a provider and is never shown as a
    //    digest mismatch.
    if let Err(over) = size::preflight(limit, signed_bytes) {
        return Settled::Failed(super::errors::FlowFailure::TooLarge {
            size: over.size,
            limit: over.limit,
        });
    }

    // 2. Choose an export destination (opaque; the browser owns the path — spec
    //    D4). A clean no-selection / dismissal is a cancel: nothing was written.
    let target = match files.pick_export_target(suggested, ct).await {
        Ok(Some(target)) => target,
        Ok(None) => return Settled::Cancelled,
        Err(error) => return Settled::from_stop(FlowStop::from_capability(error)),
    };

    // 3. `file.fetch` (op_id-addressed so `transfer.cancel` can name it). A cancel
    //    here issues `transfer.cancel { transfer_op_id }` and drops the fetch.
    let op_id = fetch_op_id(&room_id, &file_id);
    let fetch = handle.call::<FileFetch>(
        FileFetch {
            room_id: room_id.clone(),
            file_id: file_id.clone(),
        },
        Dedup::Key(op_id.clone()),
    );
    match race_cancel_call(fetch, ct, || {}).await {
        Ok(_out) => {}
        Err(FlowStop::Cancelled) => {
            // op_id-addressed cancel of the in-flight transfer (best-effort;
            // survives reconnect). The export target is discarded unwritten.
            let _ = handle
                .call::<TransferCancel>(
                    TransferCancel {
                        transfer_op_id: op_id,
                    },
                    Dedup::None,
                )
                .await;
            let _ = files.discard_export_target(&target).await;
            return Settled::Cancelled;
        }
        Err(stop) => {
            let _ = files.discard_export_target(&target).await;
            return Settled::from_stop(stop);
        }
    }

    // 4. Open the export sink and stream `file.read` into it. On the terminal,
    //    commit (completing the download); a cancel aborts the stream in-band and
    //    drops the sink uncommitted (drop-is-abort deletes the partial artifact).
    let sink = match files.export_sink(&target, ct).await {
        Ok(sink) => sink,
        Err(error) => return Settled::from_stop(FlowStop::from_capability(error)),
    };
    let mut read = handle.call_stream::<FileRead>(FileRead { room_id, file_id }, Dedup::None);
    match race_cancel_stream(&mut read, ct).await {
        Ok(_header) => {
            // (Depth deferred: the seam does not yet pump DATA into the sink.)
            match sink.commit().await {
                Ok(()) => Settled::Ok(()),
                Err(error) => Settled::from_stop(FlowStop::from_capability(error)),
            }
        }
        Err(stop) => {
            // Drop the uncommitted sink: drop-is-abort deletes the partial.
            drop(sink);
            Settled::from_stop(stop)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_op_id_is_deterministic_and_addressable() {
        let a = fetch_op_id(&RoomId::new("r"), &FileId::new("f"));
        let b = fetch_op_id(&RoomId::new("r"), &FileId::new("f"));
        assert_eq!(a, b, "a fetch's cancel must be able to name the same op");
        assert_ne!(a, fetch_op_id(&RoomId::new("r"), &FileId::new("g")));
    }
}
