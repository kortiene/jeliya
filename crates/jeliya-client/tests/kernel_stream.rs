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

/// The receiver's terminal acceptance CREDIT (protocol §Credit/END): "the
/// producer sends END only after every DATA byte it sent has been acknowledged
/// by CREDIT" — for `file.read` the daemon is the producer, and it may END
/// only once the client reports `accepted_through == total`. The client's
/// credit ceiling is capped at `total`, so once the opening window covers the
/// whole file the window can never EXTEND again — the acceptance report is
/// itself the reason to send the final CREDIT, not a window extension.
///
/// Found live: against a real jeliyad, a 64 KiB `file.read` (window 1 MiB)
/// received every DATA byte, sink-accepted all of them, and then stalled —
/// the daemon's `ready_for_end` waits for the acknowledgement this test
/// pins. The deterministic rig scripted END unconditionally, so the gap was
/// invisible until the live daemon.
#[test]
fn file_read_receiver_reports_terminal_acceptance_credit() {
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
        let wire = sent[0].id;

        // Default window (1 MiB) covers the whole 300-byte file: the opening
        // CREDIT already grants send_through = total.
        controller.open(wire, 300);
        let opening: Vec<_> = controller
            .take_outbound_records()
            .into_iter()
            .filter(|r| r.kind == "credit")
            .collect();
        assert!(!opening.is_empty(), "the opening credit is granted");
        assert!(
            opening.iter().all(|r| r.b <= 300),
            "credit within the total"
        );

        // All bytes arrive and are sink-accepted. The terminal CREDIT must
        // now report accepted_through == total (300) — even though the
        // window cannot extend past the total — or a live producer has no
        // permission path to END and the stream stalls to timeout.
        controller.deliver_data(wire, 0, 300);
        let after_data: Vec<_> = controller
            .take_outbound_records()
            .into_iter()
            .filter(|r| r.kind == "credit")
            .collect();
        assert!(
            after_data.iter().any(|r| r.a == 300),
            "the terminal acceptance CREDIT reports accepted_through == total; got {after_data:?}"
        );

        controller.end(wire, 300);
        controller.deliver_reply(
            wire,
            "{\"room_id\":\"r1\",\"file_id\":\"f1\",\"bytes\":300,\"declared_content_type\":\"application/octet-stream\"}",
        );
        let out: FileReadOut = fut.await.expect("terminal reply resolves the stream");
        assert_eq!(out.bytes, 300);
    });
    assert_eq!(controller.streams(), 0, "the stream is fully retired");
    assert_eq!(controller.stream_timers(), 0, "no stream timer survives");
    assert_eq!(controller.outstanding(), 0, "the call is fully settled");
}
