//! Happy-path lifecycle coverage for the kernel stream layer (#269 §8, AC-1/
//! AC-2): `call_stream::<FileShare>` (producer) and `call_stream::<FileRead>`
//! (receiver) drive a full OPEN → DATA/CREDIT → END → terminal-reply lifecycle
//! through the kernel against the deterministic in-memory driver. The broader
//! fault matrix (deadline, stall, credit-overshoot refusal, disconnect, cancel,
//! churn) lives in the `tests` phase's `kernel_fault.rs` extensions.
//!
//! CI runs: `cargo test -p jeliya-client --features test-transport`.

use futures::executor::block_on;
use jeliya_api::{FileId, FileRead, FileReadOut, FileShare, FileShareOut, RoomId};
use jeliya_client::{ClientHandle, Dedup, KernelConfig, KernelController, State};

fn ready() -> (ClientHandle, KernelController) {
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig::default());
    handle.start();
    controller.connect();
    assert_eq!(handle.state(), State::Ready);
    (handle, controller)
}

/// AC-1/AC-2 (producer): `file.share` drives request → OPEN → CREDIT → DATA* →
/// CREDIT → END → terminal reply, with every DATA within granted credit and END
/// only after the full byte count is acknowledged.
#[test]
fn file_share_producer_drives_full_lifecycle() {
    let (handle, controller) = ready();
    block_on(async {
        let fut = handle.call_stream::<FileShare>(
            FileShare {
                room_id: RoomId::new("r1"),
                name: String::from("a.bin"),
                declared_bytes: 200,
                declared_content_type: String::from("application/octet-stream"),
            },
            Dedup::Key(jeliya_api::OpId::new("share-1")),
        );
        let sent = controller.take_outbound();
        assert_eq!(sent.len(), 1, "the Text request reached the wire");
        let wire = sent[0].id;

        // Daemon OPENs the stream with the authoritative total.
        controller.open(wire, 200);
        // First CREDIT: granted 200, nothing accepted yet.
        controller.credit(wire, 0, 200);
        // Final CREDIT acknowledges every sent byte.
        controller.credit(wire, 200, 200);

        let records = controller.take_outbound_records();
        // Outbound DATA never exceeds the granted send_through (200), and END is
        // emitted exactly once, after full acknowledgement.
        let data: Vec<_> = records.iter().filter(|r| r.kind == "data").collect();
        assert!(!data.is_empty(), "at least one DATA record was sent");
        for record in &data {
            assert!(record.a + record.b <= 200, "DATA within granted credit");
        }
        let ends: Vec<_> = records.iter().filter(|r| r.kind == "end").collect();
        assert_eq!(ends.len(), 1, "exactly one END");
        assert_eq!(ends[0].a, 200, "END at the full total");
        assert_eq!(
            data.iter().map(|r| r.b).sum::<u64>(),
            200,
            "every byte was framed as DATA"
        );

        // The terminal Text reply settles the call.
        controller.deliver_reply(
            wire,
            "{\"room_id\":\"r1\",\"file_id\":\"f1\",\"event_id\":\"e1\",\"pos\":0,\"bytes\":200,\"digest\":\"d\"}",
        );
        let out: FileShareOut = fut.await.expect("terminal reply resolves the stream");
        assert_eq!(out.bytes, 200);
    });
    assert_eq!(controller.streams(), 0, "the stream is fully retired");
    assert_eq!(controller.stream_timers(), 0, "no stream timer survives");
    assert_eq!(controller.outstanding(), 0, "the call is fully settled");
}

/// AC-1 (receiver, zero-byte): `file.read` on a zero-byte file drives the
/// protocol's mandatory empty-file handshake OPEN(0) → CREDIT(0,0) → END(0) →
/// terminal reply. The CREDIT is required even though no window is needed: the
/// daemon's producer waits for it before it may send END, so omitting it
/// stalls an empty download until timeout. The stream retires cleanly with no
/// orphaned timers.
#[test]
fn file_read_zero_byte_receiver_complete_lifecycle() {
    let (handle, controller) = ready();
    block_on(async {
        let fut = handle.call_stream::<FileRead>(
            FileRead {
                room_id: RoomId::new("r1"),
                file_id: FileId::new("f0"),
            },
            Dedup::None,
        );
        let sent = controller.take_outbound();
        assert_eq!(sent.len(), 1);
        let wire = sent[0].id;

        controller.open(wire, 0);
        // total=0: the mandatory CREDIT(0,0) opens the zero-byte handshake.
        let credits: Vec<_> = controller
            .take_outbound_records()
            .into_iter()
            .filter(|r| r.kind == "credit")
            .collect();
        assert_eq!(credits.len(), 1, "a zero-byte file still owes CREDIT(0,0)");
        assert_eq!((credits[0].a, credits[0].b), (0, 0));

        // The daemon may now send END: offset=0, total=0.
        controller.end(wire, 0);

        controller.deliver_reply(
            wire,
            "{\"room_id\":\"r1\",\"file_id\":\"f0\",\"bytes\":0,\"declared_content_type\":\"application/octet-stream\"}",
        );
        let out: FileReadOut = fut.await.expect("zero-byte stream resolves terminal");
        assert_eq!(out.bytes, 0);
    });
    assert_eq!(
        controller.streams(),
        0,
        "stream retired after zero-byte END"
    );
    assert_eq!(controller.stream_timers(), 0, "no orphaned stream timers");
    assert_eq!(controller.outstanding(), 0, "call fully settled");
}

/// AC-1 (receiver): `file.read` drives request → OPEN → CREDIT → DATA* → END →
/// terminal reply, granting credit within the quarantine window and accepting
/// END only once the full byte sequence is sink-accepted.
#[test]
fn file_read_receiver_drives_full_lifecycle() {
    let (handle, controller) = ready();
    block_on(async {
        let fut = handle.call_stream::<FileRead>(
            FileRead {
                room_id: RoomId::new("r1"),
                file_id: FileId::new("f1"),
            },
            Dedup::None,
        );
        let sent = controller.take_outbound();
        assert_eq!(sent.len(), 1);
        let wire = sent[0].id;

        controller.open(wire, 200);
        // OPEN grants the opening credit window immediately.
        let credits: Vec<_> = controller
            .take_outbound_records()
            .into_iter()
            .filter(|r| r.kind == "credit")
            .collect();
        assert!(!credits.is_empty(), "the receiver grants credit on OPEN");
        assert!(credits.iter().all(|r| r.b <= 200), "credit within total");

        // Daemon streams all 200 bytes, then ENDs.
        controller.deliver_data(wire, 0, 200);
        controller.end(wire, 200);

        controller.deliver_reply(
            wire,
            "{\"room_id\":\"r1\",\"file_id\":\"f1\",\"bytes\":200,\"declared_content_type\":\"application/octet-stream\"}",
        );
        let out: FileReadOut = fut.await.expect("terminal reply resolves the stream");
        assert_eq!(out.bytes, 200);
    });
    assert_eq!(controller.streams(), 0, "the stream is fully retired");
    assert_eq!(controller.stream_timers(), 0, "no stream timer survives");
    assert_eq!(controller.outstanding(), 0, "the call is fully settled");
}
