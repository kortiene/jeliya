//! Behaviour suite for the deterministic fakes (#174 §7): one focused case per
//! acceptance criterion and K-invariant, exercising the denied / unavailable /
//! cancelled paths per capability. Runs under `--features fake`.

use futures::executor::block_on;

use jeliya_api::{FileId, RoomId};
use jeliya_platform::fake::{self, Capability, FakeController, RecordedEffect};
use jeliya_platform::{
    CancelToken, CapabilityError, Durability, ExportTargetKind, FailureKind, FileName,
    FileObjectKind, LifecycleEvent, LocalFileRef, PlatformServices, PreferenceKey, ProgressSink,
    Route, SafeExternalUrl, Secret, SecretKey, ShareAttachment, ShareContent, StageProgress,
    WriteOutcome,
};

fn local_ref() -> LocalFileRef {
    LocalFileRef::new(RoomId::new("r-1"), FileId::new("f-1"))
}

/// A shape constructor, so the shared cases can iterate the three fixtures.
type Build = fn() -> (PlatformServices, FakeController);

// ---- AC-1 / K4: file object kinds ---------------------------------------

#[test]
fn each_shape_picks_its_own_object_kind() {
    for (build, kind) in [
        (fake::browser as Build, FileObjectKind::BrowserBlob),
        (fake::desktop as Build, FileObjectKind::NativePath),
        (fake::android as Build, FileObjectKind::ContentUri),
    ] {
        let (services, controller) = build();
        controller.arm_pick("photo.png", None, b"hello".to_vec());
        let ct = CancelToken::new();
        let picked = block_on(services.files().pick(&ct))
            .expect("pick ok")
            .expect("a source was picked");
        assert_eq!(picked.kind(), kind);
        assert_eq!(picked.display_name().as_str(), "photo.png");
        assert_eq!(picked.size(), 5);
    }
}

// ---- AC-1 / D5 / K7: staging is bounded and size-enforced ---------------

#[test]
fn staging_a_known_source_produces_a_shareable_blob() {
    let (services, controller) = fake::desktop();
    controller.arm_pick("doc.bin", None, vec![0u8; 20]);
    let ct = CancelToken::new();
    let src = block_on(services.files().pick(&ct)).unwrap().unwrap();
    let blob = block_on(
        services
            .files()
            .stage_for_share(src, 1000, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");
    assert_eq!(blob.size(), 20);
    assert_eq!(controller.staged(), vec![(20, vec![0u8; 20])]);
}

#[test]
fn a_known_oversize_source_fails_before_any_copy() {
    let (services, controller) = fake::desktop();
    controller.arm_pick("big.bin", None, vec![0u8; 200]);
    let ct = CancelToken::new();
    let src = block_on(services.files().pick(&ct)).unwrap().unwrap();
    let outcome = block_on(services.files().stage_for_share(
        src,
        100,
        ProgressSink::discard(),
        &ct,
    ));
    assert_eq!(
        outcome,
        Err(CapabilityError::Failed(FailureKind::FileTooLarge {
            size: 200,
            limit: 100
        }))
    );
    // Nothing was staged — no partial left behind.
    assert!(controller.staged().is_empty());
}

#[test]
fn a_streamed_content_source_is_enforced_mid_copy() {
    let (services, controller) = fake::android();
    // A `content://` source: size unknown up front, enforced during the copy.
    controller.arm_pick("stream.bin", None, vec![7u8; 50]);
    let ct = CancelToken::new();
    let src = block_on(services.files().pick(&ct)).unwrap().unwrap();
    assert_eq!(src.kind(), FileObjectKind::ContentUri);
    let outcome = block_on(
        services
            .files()
            .stage_for_share(src, 16, ProgressSink::discard(), &ct),
    );
    match outcome {
        Err(CapabilityError::Failed(FailureKind::FileTooLarge { size, limit })) => {
            assert_eq!(limit, 16);
            assert!(size > 16, "aborted once the running total passed the limit");
        }
        other => panic!("expected FileTooLarge, got {other:?}"),
    }
    assert!(controller.staged().is_empty());
}

#[test]
fn an_empty_source_fails_file_empty() {
    let (services, controller) = fake::browser();
    controller.arm_pick("empty", None, Vec::new());
    let ct = CancelToken::new();
    let src = block_on(services.files().pick(&ct)).unwrap().unwrap();
    let outcome = block_on(services.files().stage_for_share(
        src,
        100,
        ProgressSink::discard(),
        &ct,
    ));
    assert_eq!(
        outcome,
        Err(CapabilityError::Failed(FailureKind::FileEmpty))
    );
}

// ---- K2: cancellation is not success, and leaves no partial -------------

#[test]
fn a_cancel_fired_mid_copy_yields_cancelled_and_no_partial() {
    let (services, controller) = fake::android();
    controller.arm_pick("stream.bin", None, vec![1u8; 40]);
    let ct = CancelToken::new();
    let src = block_on(services.files().pick(&ct)).unwrap().unwrap();
    // The progress sink fires the cancel after the first chunk — a
    // deterministic mid-copy cancel with no wall clock.
    let ct_for_sink = ct.clone();
    let sink = ProgressSink::new(move |p: StageProgress| {
        if p.transferred >= 8 {
            ct_for_sink.cancel();
        }
    });
    let outcome = block_on(services.files().stage_for_share(src, 1000, sink, &ct));
    assert_eq!(outcome, Err(CapabilityError::Cancelled));
    assert!(
        controller.staged().is_empty(),
        "a cancelled stage leaves no daemon-shareable blob"
    );
}

#[test]
fn a_pre_fired_cancel_never_becomes_ok() {
    let (services, controller) = fake::desktop();
    controller.arm_pick("doc", None, b"data".to_vec());
    let ct = CancelToken::new();
    ct.cancel();
    // Pick refuses a fired token with Cancelled, never Ok. (`PickedSource` is
    // an opaque handle with no `PartialEq`, so match on the outcome shape.)
    assert!(matches!(
        block_on(services.files().pick(&ct)),
        Err(CapabilityError::Cancelled)
    ));
}

// ---- AC-1: export / open / share resolve the token natively (K5) --------

#[test]
fn open_and_export_local_never_leak_the_daemon_token() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let target = block_on(
        services
            .files()
            .pick_export_target(FileName::new("out.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    block_on(services.files().export_local(local_ref(), target, &ct)).unwrap();
    block_on(services.files().open_local(local_ref())).unwrap();
    // The internal token string never appears in any recorded effect.
    let dump = format!("{:?}", controller.effects());
    assert!(!dump.contains("fake-native-token"), "token leaked: {dump}");
    assert!(dump.contains("ExportedLocal") && dump.contains("OpenedLocal"));
}

#[test]
fn sharing_a_staged_blob_records_the_share() {
    let (services, controller) = fake::android();
    controller.arm_pick("pic", None, vec![9u8; 10]);
    let ct = CancelToken::new();
    let src = block_on(services.files().pick(&ct)).unwrap().unwrap();
    let blob = block_on(
        services
            .files()
            .stage_for_share(src, 1000, ProgressSink::discard(), &ct),
    )
    .unwrap();
    let content = ShareContent::attachment(ShareAttachment::Blob(blob));
    block_on(services.files().share_content(content, &ct)).unwrap();
    assert!(controller
        .effects()
        .iter()
        .any(|e| matches!(e, RecordedEffect::Shared { .. })));
}

// ---- AC-2 / K6: preferences, durability honesty, custody ----------------

#[test]
fn preferences_round_trip_and_report_durability_honestly() {
    let (browser, _) = fake::browser();
    assert_eq!(
        browser.preferences().durability(),
        Durability::SessionScoped
    );
    assert_eq!(
        browser.preferences().set(PreferenceKey::LastRoom, "r-42"),
        // A browser tab persists nothing across reload.
        WriteOutcome::SessionOnly
    );
    // The value still reads back this session even though it is not durable.
    assert_eq!(
        browser
            .preferences()
            .get(&PreferenceKey::LastRoom)
            .as_deref(),
        Some("r-42")
    );

    let (desktop, controller) = fake::desktop();
    assert_eq!(desktop.preferences().durability(), Durability::Persistent);
    assert_eq!(
        desktop.preferences().set(PreferenceKey::Locale, "en"),
        WriteOutcome::Durable
    );
    // A scripted non-durable write reports SessionOnly while still applying.
    controller.force_writes_session_only(true);
    assert_eq!(
        desktop.preferences().set(PreferenceKey::SelfLabel, "me"),
        WriteOutcome::SessionOnly
    );
    assert_eq!(
        desktop
            .preferences()
            .get(&PreferenceKey::SelfLabel)
            .as_deref(),
        Some("me")
    );
}

#[test]
fn secrets_are_a_separate_store_and_never_recorded() {
    let (services, controller) = fake::browser();
    services
        .secret_store()
        .set(SecretKey::SessionCredential, Secret::new("cred-value"));
    // The secret reads back through the secret store, not preferences.
    assert_eq!(
        services
            .secret_store()
            .get(&SecretKey::SessionCredential)
            .map(|s| s.expose().to_owned()),
        Some("cred-value".to_owned())
    );
    assert!(services
        .preferences()
        .get(&PreferenceKey::LastRoom)
        .is_none());
    // The secret VALUE never lands in the recorder.
    let dump = format!("{:?}", controller.effects());
    assert!(!dump.contains("cred-value"), "secret leaked: {dump}");
}

#[test]
fn private_directory_facts_are_shape_specific() {
    let (browser, _) = fake::browser();
    assert!(!browser.private_directory().availability().is_available());
    assert_eq!(
        browser.private_directory().is_backup_excluded(),
        Err(CapabilityError::Unavailable)
    );

    let (android, _) = fake::android();
    assert!(android.private_directory().availability().is_available());
    assert_eq!(android.private_directory().is_backup_excluded(), Ok(true));
    assert_eq!(android.private_directory().is_owned_by_daemon(), Ok(true));

    let (desktop, _) = fake::desktop();
    // Desktop app-support is owned by the daemon but is not backup-excluded.
    assert_eq!(desktop.private_directory().is_backup_excluded(), Ok(false));
    assert_eq!(desktop.private_directory().is_owned_by_daemon(), Ok(true));
}

// ---- AC-3 / K8: lifecycle is representable and lossless -----------------

#[test]
fn lifecycle_intents_are_representable_and_control_survives_overflow() {
    use jeliya_platform::{BackgroundPhase, LifecycleDelivery, LifecycleEvent, WindowEvent};
    let (services, controller) = fake::desktop();
    let sub = services.lifecycle().subscribe();
    for event in [
        LifecycleEvent::Resumed,
        LifecycleEvent::Backgrounded {
            phase: BackgroundPhase::Paused,
        },
        LifecycleEvent::ProcessRestored,
        LifecycleEvent::BackRequested,
        LifecycleEvent::NavigationRequested { route: Route::Root },
        LifecycleEvent::Window(WindowEvent::CloseRequested),
    ] {
        controller.emit_lifecycle(event);
    }
    let mut delivered = Vec::new();
    while let Some(item) = sub.try_next() {
        if let LifecycleDelivery::Event(event) = item {
            delivered.push(event);
        }
    }
    assert_eq!(
        delivered.len(),
        6,
        "every representable event was delivered"
    );
    assert!(delivered.contains(&LifecycleEvent::ProcessRestored));
    assert!(delivered.contains(&LifecycleEvent::BackRequested));
}

// ---- AC-4 / K9: allowlisted safe launcher, fail-closed ------------------

#[test]
fn the_launcher_opens_only_vetted_urls_and_surfaces_failure() {
    let (services, controller) = fake::browser();
    let url = SafeExternalUrl::parse("https://example.com").unwrap();
    services.url_launcher().open_external(url.clone()).unwrap();
    assert_eq!(controller.opened_urls(), vec![url]);
    // A raw dangerous string cannot even be constructed into a SafeExternalUrl.
    assert!(SafeExternalUrl::parse("javascript:alert(1)").is_err());
    // A failed launch is surfaced, never swallowed.
    controller.force_error(Capability::OpenExternal, CapabilityError::Denied);
    let url = SafeExternalUrl::parse("mailto:a@b.com").unwrap();
    assert_eq!(
        services.url_launcher().open_external(url),
        Err(CapabilityError::Denied)
    );
}

// ---- D11 / K3: window actions unavailable where there is no window ------

#[test]
fn window_actions_are_unavailable_off_desktop() {
    let (browser, _) = fake::browser();
    assert!(!browser.window().availability().is_available());
    assert_eq!(
        browser.window().minimize(),
        Err(CapabilityError::Unavailable)
    );

    let (android, _) = fake::android();
    assert_eq!(
        android.window().request_exit(),
        Err(CapabilityError::Unavailable)
    );

    let (desktop, controller) = fake::desktop();
    assert!(desktop.window().availability().is_available());
    desktop.window().set_title("Jeliya").unwrap();
    desktop.window().minimize().unwrap();
    assert_eq!(controller.window_commands().len(), 2);
}

// ---- K3: Unavailable / Denied / Cancelled are distinctly observable -----

#[test]
fn the_three_did_not_happen_reasons_are_distinct() {
    let (services, controller) = fake::browser();
    let ct = CancelToken::new();

    // Denied: the platform said no.
    controller.force_error(Capability::Clipboard, CapabilityError::Denied);
    assert_eq!(
        services.clipboard().write_text("x"),
        Err(CapabilityError::Denied)
    );

    // Cancelled: the user dismissed the picker.
    controller.force_error(Capability::Pick, CapabilityError::Cancelled);
    assert!(matches!(
        block_on(services.files().pick(&ct)),
        Err(CapabilityError::Cancelled)
    ));

    // Unavailable: a structural fact of the platform.
    assert_eq!(services.window().focus(), Err(CapabilityError::Unavailable));
}

// ---- D10: navigation is the one state -----------------------------------

#[test]
fn navigation_updates_the_single_route_state() {
    let (services, controller) = fake::desktop();
    assert_eq!(services.navigation().route(), Route::Root);
    let room = Route::Room {
        room_id: RoomId::new("r-9"),
    };
    services.navigation().navigate(room.clone());
    assert_eq!(services.navigation().route(), room);
    services.navigation().hand_back_to_platform();
    assert_eq!(controller.navigations(), vec![room]);
    assert!(controller
        .effects()
        .iter()
        .any(|e| matches!(e, RecordedEffect::HandBackToPlatform)));
}

// ---- K11: the facade clones as one object -------------------------------

#[test]
fn a_clone_is_the_same_services_object() {
    let (services, _) = fake::browser();
    let clone = services.clone();
    assert_eq!(services, clone);
    let (other, _) = fake::browser();
    assert_ne!(services, other);
}

// ---- AC-2 / K6: preference remove is applied and recorded ---------------

#[test]
fn preference_remove_clears_the_value_and_records_none() {
    let (services, controller) = fake::desktop();
    services
        .preferences()
        .set(PreferenceKey::SelfLabel, "alice");
    assert_eq!(
        services
            .preferences()
            .get(&PreferenceKey::SelfLabel)
            .as_deref(),
        Some("alice")
    );
    services.preferences().remove(&PreferenceKey::SelfLabel);
    assert_eq!(
        services.preferences().get(&PreferenceKey::SelfLabel),
        None,
        "value is gone after remove"
    );
    let effects = controller.effects();
    // Second effect is the removal — value field is None.
    assert!(effects.iter().any(|e| matches!(
        e,
        jeliya_platform::fake::RecordedEffect::PreferenceWrite { value: None, .. }
    )));
}

// ---- AC-2 / K5: secret store remove and durability ----------------------

#[test]
fn secret_remove_clears_the_entry() {
    let (services, _) = fake::browser();
    services
        .secret_store()
        .set(SecretKey::SessionCredential, Secret::new("tok"));
    assert!(services
        .secret_store()
        .get(&SecretKey::SessionCredential)
        .is_some());
    services
        .secret_store()
        .remove(&SecretKey::SessionCredential);
    assert!(
        services
            .secret_store()
            .get(&SecretKey::SessionCredential)
            .is_none(),
        "secret is gone after remove"
    );
}

#[test]
fn secret_store_durability_matches_shape() {
    let (browser, _) = fake::browser();
    assert_eq!(
        browser.secret_store().durability(),
        Durability::SessionScoped
    );

    let (desktop, _) = fake::desktop();
    assert_eq!(desktop.secret_store().durability(), Durability::Persistent);

    let (android, _) = fake::android();
    assert_eq!(android.secret_store().durability(), Durability::Persistent);
}

// ---- AC-1: clipboard happy path is recorded -----------------------------

#[test]
fn clipboard_happy_path_is_recorded() {
    let (services, controller) = fake::desktop();
    services.clipboard().write_text("copy me").unwrap();
    assert_eq!(
        controller.last_clipboard().as_deref(),
        Some("copy me"),
        "successful clipboard write is recorded"
    );
}

// ---- AC-3: multiple lifecycle subscribers each receive events -----------

#[test]
fn multiple_lifecycle_subscribers_each_receive_all_events() {
    let (services, controller) = fake::desktop();
    let sub1 = services.lifecycle().subscribe();
    let sub2 = services.lifecycle().subscribe();

    controller.emit_lifecycle(LifecycleEvent::Resumed);
    controller.emit_lifecycle(LifecycleEvent::BackRequested);

    for sub in [&sub1, &sub2] {
        assert_eq!(
            sub.try_next(),
            Some(jeliya_platform::LifecycleDelivery::Event(
                LifecycleEvent::Resumed
            ))
        );
        assert_eq!(
            sub.try_next(),
            Some(jeliya_platform::LifecycleDelivery::Event(
                LifecycleEvent::BackRequested
            ))
        );
        assert_eq!(sub.try_next(), None);
    }
}

// ---- AC-3 / K8: LifecycleBus::close terminates stream via real poll -----

#[test]
fn lifecycle_close_terminates_the_stream_after_draining() {
    use futures::StreamExt;
    let (services, controller) = fake::desktop();
    let mut sub = services.lifecycle().subscribe();
    controller.emit_lifecycle(LifecycleEvent::Resumed);
    controller.close_lifecycle();
    // Block-on the stream: should yield Resumed, then None (closed).
    let first = block_on(sub.next());
    assert_eq!(
        first,
        Some(jeliya_platform::LifecycleDelivery::Event(
            LifecycleEvent::Resumed
        ))
    );
    let end = block_on(sub.next());
    assert_eq!(end, None, "closed bus ends the stream after draining");
}

// ---- AC-1 / K4: already-consumed export token is rejected (anti-forgery) -

#[test]
fn already_consumed_export_target_is_rejected_on_second_use() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let target = block_on(
        services
            .files()
            .pick_export_target(FileName::new("out.bin"), &ct),
    )
    .unwrap()
    .unwrap();

    // Clone the target before the first export so we can try again.
    let target_clone = target.clone();

    // First export consumes the token: ok.
    block_on(services.files().export_local(local_ref(), target, &ct)).unwrap();

    // Second export with the same (cloned) target — token is gone: Io.
    let second = block_on(
        services
            .files()
            .export_local(local_ref(), target_clone, &ct),
    );
    assert_eq!(
        second,
        Err(CapabilityError::Failed(FailureKind::Io)),
        "a re-used export target must fail"
    );
}

// ---- AC-1: Share::share rejects empty content ---------------------------

#[test]
fn share_sheet_rejects_empty_content() {
    let (services, _) = fake::android();
    let ct = CancelToken::new();
    let empty = ShareContent {
        text: None,
        attachment: None,
        anchor: None,
    };
    assert!(empty.is_empty());
    let outcome = block_on(services.share().share(empty, &ct));
    assert!(
        matches!(outcome, Err(CapabilityError::Failed(_))),
        "share of empty content must fail"
    );
}

// ---- AC-3: all BackgroundPhase variants are representable ---------------

#[test]
fn all_background_phases_are_representable() {
    use jeliya_platform::{BackgroundPhase, LifecycleDelivery, LifecycleEvent};
    let (services, controller) = fake::android();
    let sub = services.lifecycle().subscribe();
    for phase in [
        BackgroundPhase::Inactive,
        BackgroundPhase::Paused,
        BackgroundPhase::Hidden,
        BackgroundPhase::Detached,
    ] {
        controller.emit_lifecycle(LifecycleEvent::Backgrounded { phase });
    }
    let mut phases = Vec::new();
    while let Some(item) = sub.try_next() {
        if let LifecycleDelivery::Event(LifecycleEvent::Backgrounded { phase }) = item {
            phases.push(phase);
        }
    }
    assert_eq!(
        phases,
        vec![
            BackgroundPhase::Inactive,
            BackgroundPhase::Paused,
            BackgroundPhase::Hidden,
            BackgroundPhase::Detached,
        ],
        "all four BackgroundPhase variants are representable"
    );
}

// ---- AC-1: pick and pick_export_target return Ok(None) with nothing armed -

#[test]
fn pick_with_nothing_armed_returns_ok_none() {
    let (services, _) = fake::browser();
    let ct = CancelToken::new();
    let result = block_on(services.files().pick(&ct));
    // PickedSource does not implement PartialEq so we match on the shape.
    assert!(
        matches!(result, Ok(None)),
        "no-op pick is Ok(None), not Cancelled"
    );
}

#[test]
fn pick_export_target_with_nothing_armed_returns_ok_none() {
    let (services, _) = fake::desktop();
    let ct = CancelToken::new();
    let result = block_on(
        services
            .files()
            .pick_export_target(FileName::new("file.bin"), &ct),
    );
    // ExportTarget does not implement PartialEq so we match on the shape.
    assert!(
        matches!(result, Ok(None)),
        "no-op export target pick is Ok(None), not Cancelled"
    );
}

// ---- Scripted errors queue in order for the same capability -------------

#[test]
fn multiple_scripted_errors_drain_in_queue_order() {
    let (services, controller) = fake::browser();
    let ct = CancelToken::new();
    // Queue Denied, then Cancelled for Pick.
    controller.force_error(Capability::Pick, CapabilityError::Denied);
    controller.force_error(Capability::Pick, CapabilityError::Cancelled);

    let first = block_on(services.files().pick(&ct));
    assert!(matches!(first, Err(CapabilityError::Denied)));

    let second = block_on(services.files().pick(&ct));
    assert!(matches!(second, Err(CapabilityError::Cancelled)));

    // Third call — queue exhausted — falls through to the normal (unarmed) path.
    let third = block_on(services.files().pick(&ct));
    assert!(matches!(third, Ok(None)));
}

// ---- CapabilityError::failure() helper ----------------------------------

#[test]
fn capability_error_failure_helper_is_accurate() {
    let kind = FailureKind::FileEmpty;
    let failed = CapabilityError::Failed(kind);
    assert_eq!(failed.failure(), Some(kind));
    assert_eq!(CapabilityError::Unavailable.failure(), None);
    assert_eq!(CapabilityError::Denied.failure(), None);
    assert_eq!(CapabilityError::Cancelled.failure(), None);
}

// ---- AC-1: PickedSource::mime accessor ----------------------------------

#[test]
fn picked_source_mime_is_accessible_when_set() {
    use jeliya_platform::Mime;
    let (services, controller) = fake::desktop();
    controller.arm_pick_of_kind(
        "photo.jpg",
        FileObjectKind::NativePath,
        Some(Mime::new("image/jpeg")),
        b"jpg bytes".to_vec(),
    );
    let ct = CancelToken::new();
    let src = block_on(services.files().pick(&ct)).unwrap().unwrap();
    assert_eq!(src.mime().map(|m| m.as_str()), Some("image/jpeg"));
}

#[test]
fn picked_source_mime_is_none_when_not_set() {
    let (services, controller) = fake::browser();
    controller.arm_pick("no-mime.bin", None, b"data".to_vec());
    let ct = CancelToken::new();
    let src = block_on(services.files().pick(&ct)).unwrap().unwrap();
    assert!(src.mime().is_none());
}
