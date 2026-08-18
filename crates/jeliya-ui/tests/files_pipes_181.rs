//! End-to-end async-flow integration tests for #181 (Files + Pipes + browser
//! PlatformServices).  Every test drives a complete flow through in-process
//! mocks via `futures_executor::block_on` + `futures::future::join`; all
//! assertions run after both futures have settled.

use futures::future::join;
use futures_executor::block_on;
use jeliya_api::{
    ApiError, Audience, ByteTotal, CancelOutcome, DeviceId, EventId, FileFetch, FileFetchOut,
    FileId, FileRead, FileReadOut, FileShare, FileShareOut, Link, LinkReason, OpId, PipeConnect,
    PipeConnectOut, PipeId, PipePublish, PipePublishOut, PipeRelease, PipeReleaseOut, PipeRevoke,
    PipeRevokeOut, ProviderRef, RoomId, SubjectId, Target, Timestamp, TransferCancel,
    TransferCancelOut,
};
use jeliya_client::mock::{MockScript, Program};
use jeliya_platform::{
    fake::{browser, RetainedHandles},
    CancelToken, ExportTargetKind, FileName,
};
use jeliya_ui::{
    files::{
        errors::{FlowFailure, Settled},
        fetch::run_fetch_export,
        size::SizeLimit,
        transfer::progress_sink,
        upload::{run_upload, UploadDone},
    },
    pipes::{
        connect::{run_connect, run_release, run_revoke, ConnectDone},
        publish::{run_publish, PublishDone},
    },
};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Yields to the executor exactly once before resolving.
///
/// After `ct.cancel()` the flow future has not yet been re-polled (we are
/// still inside the driver's synchronous turn). Returning Poll::Pending once
/// lets `join(flow, driver)` re-poll flow FIRST (left-to-right), which drops
/// the in-flight call future (cancelling its sender) and dispatches the follow-
/// up call, BEFORE the driver reaches the next `pending_call().await`.
struct YieldOnce(bool);

impl std::future::Future for YieldOnce {
    type Output = ();
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.0 {
            std::task::Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    }
}

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

fn room_id() -> RoomId {
    RoomId::new("blake3:room")
}

fn file_id() -> FileId {
    FileId::new("blake3:file")
}

fn pipe_id() -> PipeId {
    PipeId::new("blake3:pipe")
}

fn ts() -> Timestamp {
    Timestamp::new(time::OffsetDateTime::UNIX_EPOCH)
}

fn loopback_target() -> Target {
    Target {
        host: "127.0.0.1".to_string(),
        port: 9000,
    }
}

fn no_limit() -> Option<SizeLimit> {
    None
}

fn limit(n: u64) -> Option<SizeLimit> {
    Some(SizeLimit::new(n))
}

// ---------------------------------------------------------------------------
// Upload flow tests
// ---------------------------------------------------------------------------

#[test]
fn upload_success_yields_upload_done_and_releases_blob() {
    let (services, fake_ctrl) = browser();
    fake_ctrl.arm_pick("shared.txt", None, b"hello world".to_vec());
    let share_out = FileShareOut {
        room_id: room_id(),
        file_id: file_id(),
        event_id: EventId::new("e"),
        pos: 1,
        bytes: 11,
        digest: "sha256:abc".to_string(),
    };
    let (handle, mock_ctrl) = MockScript::new()
        .on("file.share", Program::reply_ok::<FileShare>(&share_out))
        .build();
    let ct = CancelToken::new();
    let flow = run_upload(
        &handle,
        &services,
        room_id(),
        no_limit(),
        progress_sink(|_| {}),
        &ct,
    );
    let driver = async {
        fake_ctrl.pending_dialog().await;
        fake_ctrl.deliver_next();
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(
        result,
        Settled::Ok(UploadDone {
            file_id: file_id(),
            bytes: 11
        })
    );
    assert_eq!(fake_ctrl.retained_handles(), RetainedHandles::default());
}

#[test]
fn upload_too_large_refused_before_copy() {
    // A 20-byte source against a 10-byte served limit is refused in the
    // preflight step, before any staging copy takes place (spec D3).
    let (services, fake_ctrl) = browser();
    fake_ctrl.arm_pick("big.txt", None, vec![0u8; 20]);
    let (handle, _mock_ctrl) = MockScript::new().build();
    let ct = CancelToken::new();
    let flow = run_upload(
        &handle,
        &services,
        room_id(),
        limit(10),
        progress_sink(|_| {}),
        &ct,
    );
    let driver = async {
        fake_ctrl.pending_dialog().await;
        fake_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(
        result,
        Settled::Failed(FlowFailure::TooLarge {
            size: 20,
            limit: 10
        })
    );
    assert_eq!(fake_ctrl.retained_handles(), RetainedHandles::default());
}

#[test]
fn upload_empty_file_classified_as_empty() {
    // A zero-byte pick is not a TooLarge (the preflight passes: 0 ≤ any
    // limit); the staging layer catches it as FileEmpty (spec §5.B).
    let (services, fake_ctrl) = browser();
    fake_ctrl.arm_pick("empty.txt", None, b"".to_vec());
    let (handle, _mock_ctrl) = MockScript::new().build();
    let ct = CancelToken::new();
    let flow = run_upload(
        &handle,
        &services,
        room_id(),
        no_limit(),
        progress_sink(|_| {}),
        &ct,
    );
    let driver = async {
        fake_ctrl.pending_dialog().await;
        fake_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Failed(FlowFailure::Empty));
    assert_eq!(fake_ctrl.retained_handles(), RetainedHandles::default());
}

#[test]
fn upload_cancel_at_pick_settles_cancelled() {
    // Cancelling while the pick dialog is pending propagates as Cancelled —
    // never as a failure (spec D5).
    let (services, fake_ctrl) = browser();
    fake_ctrl.arm_pick("file.txt", None, b"data".to_vec());
    let (handle, _mock_ctrl) = MockScript::new().build();
    let ct = CancelToken::new();
    let flow = run_upload(
        &handle,
        &services,
        room_id(),
        no_limit(),
        progress_sink(|_| {}),
        &ct,
    );
    let driver = async {
        fake_ctrl.pending_dialog().await;
        ct.cancel();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Cancelled);
}

#[test]
fn upload_cancel_during_stream_settles_cancelled_and_releases_blob() {
    // A cancel during the `file.share` stream aborts the wire in-band and
    // always releases the staged blob — even for a cancel (spec D5).
    let (services, fake_ctrl) = browser();
    fake_ctrl.arm_pick("upload.bin", None, b"12345678".to_vec());
    let (handle, mock_ctrl) = MockScript::new()
        .on("file.share", Program::hang(false))
        .build();
    let ct = CancelToken::new();
    let flow = run_upload(
        &handle,
        &services,
        room_id(),
        no_limit(),
        progress_sink(|_| {}),
        &ct,
    );
    let driver = async {
        fake_ctrl.pending_dialog().await;
        fake_ctrl.deliver_next();
        mock_ctrl.pending_call().await;
        ct.cancel();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Cancelled);
    assert_eq!(fake_ctrl.retained_handles(), RetainedHandles::default());
}

#[test]
fn upload_wire_error_classified_and_blob_released() {
    // A wire error from `file.share` maps to the typed FlowFailure; the
    // staged blob is still released (release-always invariant, spec D5).
    let (services, fake_ctrl) = browser();
    fake_ctrl.arm_pick("upload.txt", None, b"data".to_vec());
    let (handle, mock_ctrl) = MockScript::new()
        .on(
            "file.share",
            Program::reply_err(ApiError::ProviderUnreachable {
                file_id: file_id(),
                providers: vec![],
            }),
        )
        .build();
    let ct = CancelToken::new();
    let flow = run_upload(
        &handle,
        &services,
        room_id(),
        no_limit(),
        progress_sink(|_| {}),
        &ct,
    );
    let driver = async {
        fake_ctrl.pending_dialog().await;
        fake_ctrl.deliver_next();
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Failed(FlowFailure::NoReachableProvider));
    assert_eq!(fake_ctrl.retained_handles(), RetainedHandles::default());
}

// ---------------------------------------------------------------------------
// Fetch / export flow tests
// ---------------------------------------------------------------------------

#[test]
fn fetch_success_settles_ok_and_discards_export_target() {
    let (services, fake_ctrl) = browser();
    fake_ctrl.arm_export_target(ExportTargetKind::BrowserDownload, "fetched.txt");
    let fetch_out = FileFetchOut {
        room_id: room_id(),
        file_id: file_id(),
        bytes: 8,
        digest: "sha256:xyz".to_string(),
        provider: ProviderRef {
            subject_id: SubjectId::new("s"),
            device_id: DeviceId::new("d"),
        },
    };
    let read_out = FileReadOut {
        room_id: room_id(),
        file_id: file_id(),
        bytes: 8,
        declared_content_type: "application/octet-stream".to_string(),
    };
    let (handle, mock_ctrl) = MockScript::new()
        .on("file.fetch", Program::reply_ok::<FileFetch>(&fetch_out))
        .on("file.read", Program::reply_ok::<FileRead>(&read_out))
        .build();
    let ct = CancelToken::new();
    let flow = run_fetch_export(
        &handle,
        &services,
        room_id(),
        file_id(),
        8,
        FileName::parse("fetched.txt").unwrap(),
        no_limit(),
        &ct,
    );
    let driver = async {
        fake_ctrl.pending_dialog().await;
        fake_ctrl.deliver_next();
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Ok(()));
    assert_eq!(fake_ctrl.retained_handles(), RetainedHandles::default());
}

#[test]
fn fetch_too_large_refused_before_any_provider_contact() {
    // The signed size from `file.list` is preflighted before the export
    // target dialog or `file.fetch` round trip (spec D3).
    let (services, _fake_ctrl) = browser();
    let (handle, _mock_ctrl) = MockScript::new().build();
    let ct = CancelToken::new();
    let flow = run_fetch_export(
        &handle,
        &services,
        room_id(),
        file_id(),
        200,
        FileName::parse("big.txt").unwrap(),
        limit(100),
        &ct,
    );
    let result = block_on(flow);
    assert_eq!(
        result,
        Settled::Failed(FlowFailure::TooLarge {
            size: 200,
            limit: 100
        })
    );
}

#[test]
fn fetch_cancel_at_file_fetch_issues_transfer_cancel() {
    // A cancel during `file.fetch` issues `transfer.cancel` with the same
    // op_id so the daemon can abort the in-flight transfer across a reconnect
    // (spec D5). The export target is discarded unwritten.
    let (services, fake_ctrl) = browser();
    fake_ctrl.arm_export_target(ExportTargetKind::BrowserDownload, "file.bin");
    let cancel_out = TransferCancelOut {
        transfer_op_id: OpId::new("file.fetch:blake3:room:blake3:file"),
        outcome: CancelOutcome::Cancelled,
        transferred_bytes: 0,
        total: ByteTotal::Unknown,
    };
    let (handle, mock_ctrl) = MockScript::new()
        .on("file.fetch", Program::hang(false))
        .on(
            "transfer.cancel",
            Program::reply_ok::<TransferCancel>(&cancel_out),
        )
        .build();
    let ct = CancelToken::new();
    let flow = run_fetch_export(
        &handle,
        &services,
        room_id(),
        file_id(),
        8,
        FileName::parse("file.bin").unwrap(),
        no_limit(),
        &ct,
    );
    let driver = async {
        fake_ctrl.pending_dialog().await;
        fake_ctrl.deliver_next();
        mock_ctrl.pending_call().await;
        ct.cancel();
        // Yield once so join re-polls flow first: the select in race_cancel_call
        // fires, the file.fetch call future is dropped (marking its sender
        // cancelled), and transfer.cancel is dispatched before this driver
        // resumes. Without this yield the second pending_call().await would
        // see file.fetch still in the queue (not yet cancelled) and resolve
        // immediately, after which deliver_next() would skip the hanging entry
        // and exit — leaving transfer.cancel with no one to deliver it.
        YieldOnce(false).await;
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Cancelled);
    assert_eq!(fake_ctrl.retained_handles(), RetainedHandles::default());
}

#[test]
fn fetch_file_unknown_classified_and_export_target_discarded() {
    // A `file_unknown` error from `file.fetch` maps to FileUnknown; the
    // export target chosen before the call is discarded unwritten.
    let (services, fake_ctrl) = browser();
    fake_ctrl.arm_export_target(ExportTargetKind::BrowserDownload, "file.txt");
    let (handle, mock_ctrl) = MockScript::new()
        .on(
            "file.fetch",
            Program::reply_err(ApiError::FileUnknown { file_id: file_id() }),
        )
        .build();
    let ct = CancelToken::new();
    let flow = run_fetch_export(
        &handle,
        &services,
        room_id(),
        file_id(),
        8,
        FileName::parse("file.txt").unwrap(),
        no_limit(),
        &ct,
    );
    let driver = async {
        fake_ctrl.pending_dialog().await;
        fake_ctrl.deliver_next();
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Failed(FlowFailure::FileUnknown));
    assert_eq!(fake_ctrl.retained_handles(), RetainedHandles::default());
}

#[test]
fn fetch_provider_unreachable_classified() {
    // `provider_unreachable` maps to NoReachableProvider; evidence is not
    // carried in the FlowFailure — it is read from the file.list model (spec
    // D2 / AC "honest unavailable-provider failure").
    let (services, fake_ctrl) = browser();
    fake_ctrl.arm_export_target(ExportTargetKind::BrowserDownload, "file.txt");
    let (handle, mock_ctrl) = MockScript::new()
        .on(
            "file.fetch",
            Program::reply_err(ApiError::ProviderUnreachable {
                file_id: file_id(),
                providers: vec![],
            }),
        )
        .build();
    let ct = CancelToken::new();
    let flow = run_fetch_export(
        &handle,
        &services,
        room_id(),
        file_id(),
        8,
        FileName::parse("file.txt").unwrap(),
        no_limit(),
        &ct,
    );
    let driver = async {
        fake_ctrl.pending_dialog().await;
        fake_ctrl.deliver_next();
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Failed(FlowFailure::NoReachableProvider));
}

#[test]
fn fetch_cancel_at_read_settles_cancelled() {
    // A cancel during the `file.read` stream aborts it in-band (no
    // transfer.cancel — stream abort IS the in-band cancel for file.read).
    let (services, fake_ctrl) = browser();
    fake_ctrl.arm_export_target(ExportTargetKind::BrowserDownload, "file.txt");
    let fetch_out = FileFetchOut {
        room_id: room_id(),
        file_id: file_id(),
        bytes: 8,
        digest: "sha256:abc".to_string(),
        provider: ProviderRef {
            subject_id: SubjectId::new("s"),
            device_id: DeviceId::new("d"),
        },
    };
    let (handle, mock_ctrl) = MockScript::new()
        .on("file.fetch", Program::reply_ok::<FileFetch>(&fetch_out))
        .on("file.read", Program::hang(false))
        .build();
    let ct = CancelToken::new();
    let flow = run_fetch_export(
        &handle,
        &services,
        room_id(),
        file_id(),
        8,
        FileName::parse("file.txt").unwrap(),
        no_limit(),
        &ct,
    );
    let driver = async {
        fake_ctrl.pending_dialog().await;
        fake_ctrl.deliver_next();
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
        mock_ctrl.pending_call().await;
        ct.cancel();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Cancelled);
}

// ---------------------------------------------------------------------------
// Pipe mutation tests
// ---------------------------------------------------------------------------

#[test]
fn pipe_connect_success() {
    let connect_out = PipeConnectOut {
        room_id: room_id(),
        pipe_id: pipe_id(),
        connection_id: "conn-1".to_string(),
        local: loopback_target(),
    };
    let (handle, mock_ctrl) = MockScript::new()
        .on(
            "pipe.connect",
            Program::reply_ok::<PipeConnect>(&connect_out),
        )
        .build();
    let ct = CancelToken::new();
    let flow = run_connect(&handle, room_id(), pipe_id(), &ct);
    let driver = async {
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(
        result,
        Settled::Ok(ConnectDone {
            connection_id: "conn-1".to_string(),
            local: loopback_target()
        })
    );
}

#[test]
fn pipe_connect_unreachable_distinct_from_unknown() {
    // `pipe_unreachable` (offline publisher) maps to PipeUnreachable — the
    // distinctive failure; `pipe_unknown` maps to PipeUnknown. The two MUST
    // NOT be conflated (spec D2 / #94).
    let (handle, mock_ctrl) = MockScript::new()
        .on(
            "pipe.connect",
            Program::reply_err(ApiError::PipeUnreachable {
                pipe_id: pipe_id(),
                link: Link::NotConnected {
                    reason: LinkReason::NoRoute,
                },
            }),
        )
        .build();
    let ct = CancelToken::new();
    let flow = run_connect(&handle, room_id(), pipe_id(), &ct);
    let driver = async {
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Failed(FlowFailure::PipeUnreachable));
    assert_ne!(result, Settled::Failed(FlowFailure::PipeUnknown));
}

#[test]
fn pipe_connect_cancel_settles_cancelled() {
    let (handle, mock_ctrl) = MockScript::new()
        .on("pipe.connect", Program::hang(false))
        .build();
    let ct = CancelToken::new();
    let flow = run_connect(&handle, room_id(), pipe_id(), &ct);
    let driver = async {
        mock_ctrl.pending_call().await;
        ct.cancel();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Cancelled);
}

#[test]
fn pipe_release_success() {
    let release_out = PipeReleaseOut {
        connection_id: "conn-1".to_string(),
        released: true,
    };
    let (handle, mock_ctrl) = MockScript::new()
        .on(
            "pipe.release",
            Program::reply_ok::<PipeRelease>(&release_out),
        )
        .build();
    let ct = CancelToken::new();
    let flow = run_release(&handle, "conn-1".to_string(), &ct);
    let driver = async {
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Ok(()));
}

#[test]
fn pipe_revoke_success() {
    let revoke_out = PipeRevokeOut {
        room_id: room_id(),
        pipe_id: pipe_id(),
        event_id: EventId::new("e"),
        pos: 1,
        revoked_at: ts(),
    };
    let (handle, mock_ctrl) = MockScript::new()
        .on("pipe.revoke", Program::reply_ok::<PipeRevoke>(&revoke_out))
        .build();
    let ct = CancelToken::new();
    let flow = run_revoke(&handle, room_id(), pipe_id(), &ct);
    let driver = async {
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Ok(()));
}

#[test]
fn pipe_publish_loopback_refused_locally_no_call() {
    // A non-loopback target is refused client-side before any `pipe.publish`
    // round trip — the daemon never sees it (spec §5.C D6/D7).
    let (handle, _mock_ctrl) = MockScript::new().build();
    let ct = CancelToken::new();
    let target = Target {
        host: "192.168.1.1".to_string(),
        port: 8080,
    };
    let result = block_on(run_publish(&handle, room_id(), target, Audience::Room, &ct));
    assert_eq!(result, Settled::Failed(FlowFailure::PipeTargetRefused));
}

#[test]
fn pipe_publish_success() {
    let publish_out = PipePublishOut {
        room_id: room_id(),
        pipe_id: pipe_id(),
        target: loopback_target(),
        audience: Audience::Room,
        event_id: EventId::new("e"),
        pos: 1,
    };
    let (handle, mock_ctrl) = MockScript::new()
        .on(
            "pipe.publish",
            Program::reply_ok::<PipePublish>(&publish_out),
        )
        .build();
    let ct = CancelToken::new();
    let flow = run_publish(&handle, room_id(), loopback_target(), Audience::Room, &ct);
    let driver = async {
        mock_ctrl.pending_call().await;
        mock_ctrl.deliver_next();
    };
    let (result, ()) = block_on(join(flow, driver));
    assert_eq!(result, Settled::Ok(PublishDone { pipe_id: pipe_id() }));
}
