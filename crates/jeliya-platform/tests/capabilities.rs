//! Behaviour suite for the deterministic fakes (#174 §7): one focused case per
//! acceptance criterion and K-invariant, exercising the denied / unavailable /
//! cancelled paths per capability. Runs under `--features fake`.

use std::task::Poll;

use futures::executor::block_on;

use jeliya_api::RoomId;
use jeliya_platform::fake::{self, Capability, FakeController, RecordedEffect};
use jeliya_platform::{
    CancelToken, CapabilityError, Durability, ExportTargetKind, FailureKind, FileName,
    FileObjectKind, LifecycleEvent, Mime, PlatformServices, PreferenceKey, ProgressSink, RoomDest,
    Route, SafeExternalUrl, Secret, SecretKey, ShareAttachment, ShareContent, StageProgress,
    WriteOutcome,
};

/// A shape constructor, so the shared cases can iterate the three fixtures.
type Build = fn() -> (PlatformServices, FakeController);

/// Drive a dialog-gated operation to its settled outcome: poll (opening the
/// dialog), advance the controller whenever it parks — the terse path for
/// tests that don't assert on in-flight ordering.
fn settle<F: std::future::Future>(controller: &FakeController, fut: F) -> F::Output {
    block_on(async move {
        let mut fut = std::pin::pin!(fut);
        loop {
            if let Poll::Ready(out) = futures::poll!(fut.as_mut()) {
                return out;
            }
            assert!(
                controller.deliver_next(),
                "operation parked but no dialog is deliverable"
            );
        }
    })
}

/// A validated [`FileName`] for a test literal. `FileName::parse` is the sole
/// constructor and fails closed on path syntax; a test literal is authored, so
/// an invalid one is a test bug worth panicking on.
fn name(literal: &str) -> FileName {
    FileName::parse(literal).expect("valid file name")
}

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
        let picked = settle(&controller, services.files().pick(&ct))
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
    let src = settle(&controller, services.files().pick(&ct))
        .unwrap()
        .unwrap();
    let blob = block_on(
        services
            .files()
            .stage_for_share(&src, 1000, ProgressSink::discard(), &ct),
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
    let src = settle(&controller, services.files().pick(&ct))
        .unwrap()
        .unwrap();
    let outcome = block_on(services.files().stage_for_share(
        &src,
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
    let src = settle(&controller, services.files().pick(&ct))
        .unwrap()
        .unwrap();
    assert_eq!(src.kind(), FileObjectKind::ContentUri);
    let outcome = block_on(services.files().stage_for_share(
        &src,
        16,
        ProgressSink::discard(),
        &ct,
    ));
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
    let src = settle(&controller, services.files().pick(&ct))
        .unwrap()
        .unwrap();
    let outcome = block_on(services.files().stage_for_share(
        &src,
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
    let src = settle(&controller, services.files().pick(&ct))
        .unwrap()
        .unwrap();
    // The progress sink fires the cancel after the first chunk — a
    // deterministic mid-copy cancel with no wall clock.
    let ct_for_sink = ct.clone();
    let sink = ProgressSink::new(move |p: StageProgress| {
        if p.transferred >= 8 {
            ct_for_sink.cancel();
        }
    });
    let outcome = block_on(services.files().stage_for_share(&src, 1000, sink, &ct));
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
    assert!(
        controller.open_dialogs().is_empty(),
        "a pre-fired token never opens a dialog"
    );
}

// ---- AC-1: export / open route fetched bytes through sinks (v2) ---------

#[test]
fn export_and_open_sinks_commit_the_written_bytes() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let target = settle(
        &controller,
        services.files().pick_export_target(name("out.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    let mut sink = block_on(services.files().export_sink(&target, &ct)).unwrap();
    block_on(sink.write(vec![1, 2])).unwrap();
    block_on(sink.write(vec![3])).unwrap();
    block_on(sink.commit()).unwrap();

    let mut open = block_on(services.files().open_sink(name("view.bin"), None, &ct)).unwrap();
    block_on(open.write(vec![9])).unwrap();
    block_on(open.commit()).unwrap();

    let effects = controller.effects();
    assert!(effects.iter().any(|e| matches!(
        e,
        RecordedEffect::ExportedLocal { kind: ExportTargetKind::NativePath, bytes } if bytes == &[1, 2, 3]
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        RecordedEffect::OpenedLocal { bytes, .. } if bytes == &[9]
    )));
}

#[test]
fn dropping_an_uncommitted_export_sink_leaves_no_artifact() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let target = settle(
        &controller,
        services.files().pick_export_target(name("out.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    {
        let mut sink = block_on(services.files().export_sink(&target, &ct)).unwrap();
        block_on(sink.write(vec![1, 2])).unwrap();
        block_on(sink.write(vec![3, 4])).unwrap();
        // Dropped without commit: the partial artifact fails closed.
    }
    assert!(
        !controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::ExportedLocal { .. })),
        "a dropped, uncommitted sink records no export"
    );
    // A fresh target + sink with write+commit records the full sequence.
    controller.arm_export_target(ExportTargetKind::NativePath, "out2.bin");
    let target = settle(
        &controller,
        services.files().pick_export_target(name("out2.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    let mut sink = block_on(services.files().export_sink(&target, &ct)).unwrap();
    block_on(sink.write(vec![5, 6])).unwrap();
    block_on(sink.write(vec![7])).unwrap();
    block_on(sink.commit()).unwrap();
    assert!(controller.effects().iter().any(|e| matches!(
        e,
        RecordedEffect::ExportedLocal { bytes, .. } if bytes == &[5, 6, 7]
    )));
}

#[test]
fn sharing_a_staged_blob_records_the_share() {
    let (services, controller) = fake::android();
    controller.arm_pick("pic", None, vec![9u8; 10]);
    let ct = CancelToken::new();
    let src = settle(&controller, services.files().pick(&ct))
        .unwrap()
        .unwrap();
    let blob = block_on(
        services
            .files()
            .stage_for_share(&src, 1000, ProgressSink::discard(), &ct),
    )
    .unwrap();
    let content = ShareContent::attachment(ShareAttachment::Blob(blob));
    settle(&controller, services.files().share_content(&content, &ct)).unwrap();
    assert!(controller
        .effects()
        .iter()
        .any(|e| matches!(e, RecordedEffect::Shared { .. })));
}

// ---- v2 upload plumbing: staged bytes are pullable under a bound ---------

#[test]
fn read_staged_pulls_the_staged_bytes_boundedly() {
    let (services, controller) = fake::desktop();
    controller.arm_pick("doc.bin", None, (0u8..20).collect());
    let ct = CancelToken::new();
    let src = settle(&controller, services.files().pick(&ct))
        .unwrap()
        .unwrap();
    let blob = block_on(
        services
            .files()
            .stage_for_share(&src, 1000, ProgressSink::discard(), &ct),
    )
    .unwrap();
    let mut reader = block_on(services.files().read_staged(&blob)).unwrap();
    assert_eq!(reader.size(), blob.size());
    let mut pulled = Vec::new();
    while let Some(chunk) = block_on(reader.next_chunk(3)).unwrap() {
        assert!(chunk.len() <= 3, "every pull respects the caller's bound");
        pulled.extend_from_slice(&chunk);
    }
    let staged = controller.staged();
    assert_eq!(staged.len(), 1);
    assert_eq!(
        pulled, staged[0].1,
        "the pulled concatenation byte-equals the staged bytes"
    );
}

#[test]
fn read_staged_fails_closed_on_a_blob_this_service_never_minted() {
    // Stage a blob on a SECOND fake; the first service must refuse to read
    // it — never an empty Ok reader.
    let (foreign, foreign_controller) = fake::desktop();
    foreign_controller.arm_pick("doc", None, b"foreign".to_vec());
    let ct = CancelToken::new();
    let src = settle(&foreign_controller, foreign.files().pick(&ct))
        .unwrap()
        .unwrap();
    let foreign_blob = block_on(foreign.files().stage_for_share(
        &src,
        1000,
        ProgressSink::discard(),
        &ct,
    ))
    .unwrap();

    let (services, _controller) = fake::desktop();
    assert_eq!(
        block_on(services.files().read_staged(&foreign_blob)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "an unminted blob token fails closed"
    );
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
        desktop.preferences().set(PreferenceKey::TextLocale, "en"),
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

/// The text language and the formatting locale are two independent rows, not
/// one conflated key: the product contract fixes "text locale != formatting
/// locale from day one" (a Bambara UI on a French system formats dates the
/// French way), so choosing both must not make the second overwrite the first.
#[test]
fn text_and_formatting_locales_are_independent_preferences() {
    let (desktop, _) = fake::desktop();
    let prefs = desktop.preferences();
    assert_eq!(
        prefs.set(PreferenceKey::TextLocale, "bm"),
        WriteOutcome::Durable
    );
    assert_eq!(
        prefs.set(PreferenceKey::FormattingLocale, "fr-FR"),
        WriteOutcome::Durable
    );
    // Both survive side by side — the state a single `Locale` key could not
    // represent at all.
    assert_eq!(prefs.get(&PreferenceKey::TextLocale).as_deref(), Some("bm"));
    assert_eq!(
        prefs.get(&PreferenceKey::FormattingLocale).as_deref(),
        Some("fr-FR")
    );
    // Clearing one leaves the other intact.
    prefs.remove(&PreferenceKey::TextLocale);
    assert_eq!(prefs.get(&PreferenceKey::TextLocale), None);
    assert_eq!(
        prefs.get(&PreferenceKey::FormattingLocale).as_deref(),
        Some("fr-FR"),
        "removing the text locale must not disturb the formatting locale"
    );
}

/// `force_writes_session_only` is a durability-regime **latch**, not a one-shot
/// script entry: it models storage that stopped persisting, so it spans every
/// store and holds until cleared — and recovery is observable through the same
/// public toggle rather than any hidden reset.
#[test]
fn forced_session_only_is_a_latch_until_cleared() {
    let (desktop, controller) = fake::desktop();
    controller.force_writes_session_only(true);
    assert_eq!(
        desktop.preferences().set(PreferenceKey::SelfLabel, "me"),
        WriteOutcome::SessionOnly
    );
    // The second write is still session-only: the latch spans preferences and
    // secrets alike and is not consumed by the first write.
    assert_eq!(
        desktop
            .secret_store()
            .set(SecretKey::SessionCredential, Secret::new("c")),
        WriteOutcome::SessionOnly
    );
    controller.force_writes_session_only(false);
    assert_eq!(
        desktop.preferences().set(PreferenceKey::SelfLabel, "me"),
        WriteOutcome::Durable,
        "clearing the latch restores durable writes"
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
        LifecycleEvent::NavigationRequested {
            route: Route::Rooms,
        },
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

    // Denied: the platform said no. The denial arrives through the resolved
    // future — a clipboard write settles asynchronously, never as a
    // pre-settled success.
    controller.force_error(Capability::Clipboard, CapabilityError::Denied);
    assert_eq!(
        block_on(services.clipboard().write_text("x")),
        Err(CapabilityError::Denied),
        "clipboard denial must arrive through the resolved future"
    );

    // Cancelled: the user dismissed the picker.
    controller.force_error(Capability::Pick, CapabilityError::Cancelled);
    assert!(matches!(
        settle(&controller, services.files().pick(&ct)),
        Err(CapabilityError::Cancelled)
    ));

    // Unavailable: a structural fact of the platform.
    assert_eq!(services.window().focus(), Err(CapabilityError::Unavailable));
}

// ---- D10: navigation is the one state -----------------------------------

#[test]
fn navigation_updates_the_single_route_state() {
    let (services, controller) = fake::desktop();
    assert_eq!(services.navigation().route(), Route::Rooms);
    let room = Route::Room {
        room_id: RoomId::new("r-9"),
        dest: RoomDest::Activity,
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
    block_on(services.clipboard().write_text("copy me")).unwrap();
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
    let target = settle(
        &controller,
        services.files().pick_export_target(name("out.bin"), &ct),
    )
    .unwrap()
    .unwrap();

    // Clone the target before the first sink so we can try again.
    let target_clone = target.clone();

    // First sink consumes the token: ok.
    let sink = block_on(services.files().export_sink(&target, &ct)).unwrap();
    block_on(sink.commit()).unwrap();

    // Second sink with the same (cloned) target — token is gone: Io.
    let second = block_on(services.files().export_sink(&target_clone, &ct)).err();
    assert_eq!(
        second,
        Some(CapabilityError::Failed(FailureKind::Io)),
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
    let outcome = block_on(services.share().share(&empty, &ct));
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
    let (services, controller) = fake::browser();
    let ct = CancelToken::new();
    let result = settle(&controller, services.files().pick(&ct));
    // PickedSource does not implement PartialEq so we match on the shape.
    assert!(
        matches!(result, Ok(None)),
        "no-op pick is Ok(None), not Cancelled"
    );
}

#[test]
fn pick_export_target_with_nothing_armed_returns_ok_none() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    let result = settle(
        &controller,
        services.files().pick_export_target(name("file.bin"), &ct),
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

    let first = settle(&controller, services.files().pick(&ct));
    assert!(matches!(first, Err(CapabilityError::Denied)));

    let second = settle(&controller, services.files().pick(&ct));
    assert!(matches!(second, Err(CapabilityError::Cancelled)));

    // Third call — queue exhausted — falls through to the normal (unarmed) path.
    let third = settle(&controller, services.files().pick(&ct));
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
    let src = settle(&controller, services.files().pick(&ct))
        .unwrap()
        .unwrap();
    assert_eq!(src.mime().map(|m| m.as_str()), Some("image/jpeg"));
}

#[test]
fn picked_source_mime_is_none_when_not_set() {
    let (services, controller) = fake::browser();
    controller.arm_pick("no-mime.bin", None, b"data".to_vec());
    let ct = CancelToken::new();
    let src = settle(&controller, services.files().pick(&ct))
        .unwrap()
        .unwrap();
    assert!(src.mime().is_none());
}

// ---- §6 Determinism: dialogs settle only on controller advancement ------

#[test]
fn an_armed_pick_stays_open_until_the_controller_delivers() {
    let (services, controller) = fake::desktop();
    controller.arm_pick("doc", None, b"data".to_vec());
    let ct = CancelToken::new();
    block_on(async {
        let mut fut = std::pin::pin!(services.files().pick(&ct));
        assert!(
            futures::poll!(fut.as_mut()).is_pending(),
            "spec §6: a scripted picker resolves only when the test advances it"
        );
        assert!(
            controller.deliver_next(),
            "one dialog is open and deliverable"
        );
        let picked = fut.await.expect("pick ok").expect("a source was picked");
        assert_eq!(picked.display_name().as_str(), "doc");
    });
    assert!(!controller.deliver_next(), "nothing left to deliver");
}

#[test]
fn an_armed_export_pick_stays_open_until_the_controller_delivers() {
    let (services, controller) = fake::desktop();
    controller.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let ct = CancelToken::new();
    block_on(async {
        let mut fut = std::pin::pin!(services.files().pick_export_target(name("out.bin"), &ct));
        assert!(
            futures::poll!(fut.as_mut()).is_pending(),
            "the save dialog stays open until advanced"
        );
        assert!(controller.deliver_next());
        assert!(fut.await.expect("pick ok").is_some());
    });
}

#[test]
fn a_share_sheet_stays_open_until_the_controller_delivers() {
    let (services, controller) = fake::android();
    controller.arm_pick("pic", None, vec![9u8; 10]);
    let ct = CancelToken::new();
    let src = settle(&controller, services.files().pick(&ct))
        .unwrap()
        .unwrap();
    let blob = block_on(
        services
            .files()
            .stage_for_share(&src, 1000, ProgressSink::discard(), &ct),
    )
    .unwrap();
    block_on(async {
        let content = ShareContent::attachment(ShareAttachment::Blob(blob));
        let mut fut = std::pin::pin!(services.files().share_content(&content, &ct));
        assert!(
            futures::poll!(fut.as_mut()).is_pending(),
            "the share sheet stays open until advanced"
        );
        assert!(controller.deliver_next());
        fut.await.expect("share ok");
    });
}

#[test]
fn a_cancel_while_the_picker_is_open_beats_a_later_delivery() {
    let (services, controller) = fake::desktop();
    controller.arm_pick("doc", None, b"data".to_vec());
    let ct = CancelToken::new();
    block_on(async {
        let mut fut = std::pin::pin!(services.files().pick(&ct));
        assert!(
            futures::poll!(fut.as_mut()).is_pending(),
            "the dialog is open"
        );
        ct.cancel();
        controller.deliver_next();
        assert!(
            matches!(fut.await, Err(CapabilityError::Cancelled)),
            "cancellation wins the race"
        );
    });
    assert!(
        !controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::Picked { .. })),
        "a cancelled dialog records no pick"
    );
}

#[test]
fn dropping_an_open_dialog_withdraws_it() {
    let (services, controller) = fake::desktop();
    controller.arm_pick("doc", None, b"data".to_vec());
    let ct = CancelToken::new();
    block_on(async {
        let mut fut = std::pin::pin!(services.files().pick(&ct));
        assert!(futures::poll!(fut.as_mut()).is_pending());
        assert_eq!(controller.open_dialogs().len(), 1);
    }); // future dropped here
    assert!(
        controller.open_dialogs().is_empty(),
        "a dropped dialog future withdraws its dialog"
    );
    assert!(
        !controller.deliver_next(),
        "nothing is deliverable after the drop"
    );
    assert!(!controller
        .effects()
        .iter()
        .any(|e| matches!(e, RecordedEffect::Picked { .. })));
}

#[test]
fn outcomes_bind_at_call_time_and_deliver_in_call_order() {
    let (services, controller) = fake::browser();
    let ct = CancelToken::new();
    controller.force_error(Capability::Pick, CapabilityError::Denied);
    controller.arm_pick("doc", None, b"x".to_vec());
    block_on(async {
        let mut first = std::pin::pin!(services.files().pick(&ct));
        let mut second = std::pin::pin!(services.files().pick(&ct));
        assert!(futures::poll!(first.as_mut()).is_pending());
        assert!(futures::poll!(second.as_mut()).is_pending());
        assert!(controller.deliver_next());
        assert!(controller.deliver_next());
        // Poll in REVERSE order: each outcome stays with its call.
        assert!(matches!(
            futures::poll!(second.as_mut()),
            Poll::Ready(Ok(Some(_)))
        ));
        assert!(matches!(
            futures::poll!(first.as_mut()),
            Poll::Ready(Err(CapabilityError::Denied))
        ));
    });
}

#[test]
fn pending_dialog_resolves_when_a_dialog_opens_and_unregisters_on_drop() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    block_on(async {
        let mut waiter = std::pin::pin!(controller.pending_dialog());
        assert!(
            futures::poll!(waiter.as_mut()).is_pending(),
            "no dialog open yet"
        );
        controller.arm_pick("doc", None, b"d".to_vec());
        let mut fut = std::pin::pin!(services.files().pick(&ct));
        assert!(futures::poll!(fut.as_mut()).is_pending());
        assert!(
            futures::poll!(waiter.as_mut()).is_ready(),
            "a dialog is now open"
        );
        controller.deliver_next();
        assert!(matches!(fut.await, Ok(Some(_))));
    });
    // A dropped waiter leaves no parked waker behind.
    block_on(async {
        let mut waiter = std::pin::pin!(controller.pending_dialog());
        assert!(futures::poll!(waiter.as_mut()).is_pending());
    });
    assert!(!controller.deliver_next());
}

// ---- K4: the implementation factory cannot bypass the minted-token gate --

/// Compiled only when BOTH `fake` and `implementation` are on (the factory
/// surface plus a service to aim it at): a blob forged through the factory
/// with a token this service never minted fails closed at resolution —
/// never `Ok`, and never an open share sheet.
#[cfg(feature = "implementation")]
#[test]
fn a_factory_blob_with_an_unminted_token_fails_closed() {
    use jeliya_platform::implementation::{blob_token_from_raw, shareable_blob};
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    // 7777 has a zero high half, so no minted token can ever equal it: service
    // ids start at 1 and occupy the high 32 bits.
    let forged = shareable_blob(blob_token_from_raw(7777), 10);
    let content = ShareContent::attachment(ShareAttachment::Blob(forged.clone()));
    assert_eq!(
        block_on(services.files().share_content(&content, &ct)),
        Err(CapabilityError::Failed(FailureKind::Unreadable)),
        "a forged blob never opens the share sheet"
    );
    assert!(controller.open_dialogs().is_empty());
    assert!(
        matches!(
            block_on(services.files().read_staged(&forged)).err(),
            Some(CapabilityError::Failed(FailureKind::Unreadable))
        ),
        "a forged blob is unreadable too"
    );
}

// ---- K4: tokens carry issuer provenance ---------------------------------

/// A handle from one service must never resolve in another's registries. Each
/// fake counts from 1, so without issuer provenance two services running the
/// same call sequence mint identical tokens and A's blob silently reads B's
/// bytes — the exact opposite of "a blob the service did not stage can neither
/// be read nor shared".
#[test]
fn cross_service_tokens_fail_closed() {
    let (a, a_ctl) = fake::desktop();
    let (b, b_ctl) = fake::desktop();
    let ct = CancelToken::new();

    // Identical call sequences on both services.
    let mut staged = Vec::new();
    for (services, controller) in [(&a, &a_ctl), (&b, &b_ctl)] {
        controller.arm_pick("doc.bin", None, b"abcdefgh".to_vec());
        let src = settle(controller, services.files().pick(&ct))
            .expect("pick ok")
            .expect("a source was picked");
        let blob = settle(
            controller,
            services
                .files()
                .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
        )
        .expect("stage ok");
        staged.push(blob);
    }
    let blob_a = staged.remove(0);

    assert_eq!(
        block_on(b.files().read_staged(&blob_a)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "service B must not resolve service A's blob token"
    );
    assert_eq!(
        block_on(b.files().share_content(
            &ShareContent::attachment(ShareAttachment::Blob(blob_a)),
            &ct
        )),
        Err(CapabilityError::Failed(FailureKind::Unreadable)),
        "service B must not share service A's blob"
    );
    assert!(
        b_ctl.open_dialogs().is_empty(),
        "a cross-service attachment never opens the share sheet"
    );

    // The same provenance rule covers sources and export targets.
    a_ctl.arm_pick("other.bin", None, b"xyz".to_vec());
    let src_a = settle(&a_ctl, a.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    assert_eq!(
        block_on(
            b.files()
                .stage_for_share(&src_a, 1024, ProgressSink::discard(), &ct)
        )
        .err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "service B must not stage service A's picked source"
    );
    a_ctl.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let target_a = settle(&a_ctl, a.files().pick_export_target(name("out.bin"), &ct))
        .expect("pick_export_target ok")
        .expect("a target was picked");
    assert_eq!(
        block_on(b.files().export_sink(&target_a, &ct)).err(),
        Some(CapabilityError::Failed(FailureKind::Io)),
        "service B must not write to service A's export target"
    );
}

// ---- Determinism: wakers never run under a fake's own lock --------------

/// An executor that polls inline from `wake()` must not deadlock the fake. The
/// waker here re-enters the dialog queue exactly as an inline re-poll would;
/// pre-fix, `deliver_next` invoked it while still holding that lock.
#[test]
fn dialog_wakers_run_after_the_queue_lock_is_released() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Wake, Waker};

    struct ReenteringWaker {
        controller: FakeController,
        wakes: Arc<AtomicUsize>,
    }

    impl Wake for ReenteringWaker {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            // Touch the queue the way an inline re-poll would: if the waker is
            // invoked under the queue lock, this re-lock deadlocks.
            let _ = self.controller.open_dialogs();
            self.wakes.fetch_add(1, Ordering::SeqCst);
        }
    }

    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("doc.bin", None, b"x".to_vec());
    let wakes = Arc::new(AtomicUsize::new(0));
    let waker = Waker::from(Arc::new(ReenteringWaker {
        controller: controller.clone(),
        wakes: wakes.clone(),
    }));
    let mut cx = Context::from_waker(&waker);

    let mut fut = std::pin::pin!(services.files().pick(&ct));
    assert!(
        std::future::Future::poll(fut.as_mut(), &mut cx).is_pending(),
        "the dialog parks on first poll"
    );
    // Releases the dialog and invokes the parked waker — after the guard drops.
    assert!(controller.deliver_next());
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        1,
        "the parked waker ran exactly once"
    );
    assert!(matches!(
        std::future::Future::poll(fut.as_mut(), &mut cx),
        Poll::Ready(Ok(Some(_)))
    ));

    // The same holds for the `pending_dialog` waiters woken by `open`.
    let mut waiter = std::pin::pin!(controller.pending_dialog());
    assert!(std::future::Future::poll(waiter.as_mut(), &mut cx).is_pending());
    controller.arm_pick("second.bin", None, b"y".to_vec());
    let mut second = std::pin::pin!(services.files().pick(&ct));
    assert!(std::future::Future::poll(second.as_mut(), &mut cx).is_pending());
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        2,
        "opening a dialog woke the parked waiter, lock-free"
    );
    assert!(std::future::Future::poll(waiter.as_mut(), &mut cx).is_ready());
    assert!(controller.deliver_next());
}

/// A released-but-unsettled dialog is already spoken for: `pending_dialog`
/// must keep parking, because `deliver_next` would find nothing to advance.
/// Resolving on it would spin a controller loop.
#[test]
fn pending_dialog_ignores_an_already_released_dialog() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("doc.bin", None, b"x".to_vec());
    block_on(async {
        let mut fut = std::pin::pin!(services.files().pick(&ct));
        assert!(futures::poll!(fut.as_mut()).is_pending());
        assert!(controller.deliver_next(), "the open dialog is deliverable");

        // The pick has NOT been re-polled, so the released entry still sits in
        // the queue.
        let mut waiter = std::pin::pin!(controller.pending_dialog());
        assert!(
            futures::poll!(waiter.as_mut()).is_pending(),
            "a released dialog is not awaiting advancement"
        );
        assert!(
            !controller.deliver_next(),
            "and deliver_next agrees there is nothing to advance"
        );

        // Settle it, then open a second dialog: the parked waiter wakes.
        assert!(matches!(fut.await, Ok(Some(_))));
        controller.arm_pick("second.bin", None, b"y".to_vec());
        let mut second = std::pin::pin!(services.files().pick(&ct));
        assert!(futures::poll!(second.as_mut()).is_pending());
        assert!(
            futures::poll!(waiter.as_mut()).is_ready(),
            "a newly opened dialog resolves the waiter"
        );
        assert!(controller.deliver_next());
        assert!(matches!(second.await, Ok(Some(_))));
    });
}

// ---- The pull side rejects a zero bound ---------------------------------

/// A credit-driven puller with zero credit must not be handed an empty chunk
/// that looks like progress: `next_chunk(0)` fails, and the read position is
/// untouched so the next bounded pull is still correct.
#[test]
fn zero_length_staged_pulls_fail_rather_than_spin() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("blob.bin", None, b"0123456789".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let blob = settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");
    let mut reader = block_on(services.files().read_staged(&blob)).expect("reader");
    assert_eq!(
        block_on(reader.next_chunk(0)).err(),
        Some(CapabilityError::Failed(FailureKind::Io)),
        "a zero bound is an error, not an empty non-EOF chunk"
    );
    assert_eq!(
        block_on(reader.next_chunk(4)).expect("bounded pull"),
        Some(b"0123".to_vec()),
        "the rejected pull consumed no position"
    );
}

// ---- Non-dialog outcomes bind at call time too --------------------------

/// The script's "next call" semantics must survive out-of-order polling for
/// the non-dialog methods as well: the outcome belongs to the call that
/// consumed it, not to whichever future is polled first.
#[test]
fn non_dialog_outcomes_bind_at_call_time() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.force_error(Capability::Clipboard, CapabilityError::Denied);
    block_on(async {
        let mut first = std::pin::pin!(services.clipboard().write_text("first"));
        let mut second = std::pin::pin!(services.clipboard().write_text("second"));
        assert!(matches!(
            futures::poll!(second.as_mut()),
            Poll::Ready(Ok(()))
        ));
        assert!(matches!(
            futures::poll!(first.as_mut()),
            Poll::Ready(Err(CapabilityError::Denied))
        ));
    });
    assert_eq!(controller.last_clipboard().as_deref(), Some("second"));

    controller.force_error(
        Capability::OpenSink,
        CapabilityError::Failed(FailureKind::Io),
    );
    block_on(async {
        let mut first = std::pin::pin!(services.files().open_sink(name("a.bin"), None, &ct));
        let mut second = std::pin::pin!(services.files().open_sink(name("b.bin"), None, &ct));
        assert!(matches!(
            futures::poll!(second.as_mut()),
            Poll::Ready(Ok(_))
        ));
        assert!(matches!(
            futures::poll!(first.as_mut()),
            Poll::Ready(Err(CapabilityError::Failed(FailureKind::Io)))
        ));
    });
}

/// A pre-fired token never starts the operation, so it consumes nothing —
/// the dispatch-after-stop rule `pick` documents, now uniform across the file.
#[test]
fn a_pre_fired_token_consumes_no_scripted_outcome() {
    let (services, controller) = fake::desktop();
    let cancelled = CancelToken::new();
    cancelled.cancel();
    controller.force_error(
        Capability::OpenSink,
        CapabilityError::Failed(FailureKind::Io),
    );
    assert_eq!(
        block_on(services.files().open_sink(name("a.bin"), None, &cancelled)).err(),
        Some(CapabilityError::Cancelled),
        "cancellation wins and the operation never starts"
    );
    // The scripted error is still queued for the call that does start.
    let ct = CancelToken::new();
    assert_eq!(
        block_on(services.files().open_sink(name("a.bin"), None, &ct)).err(),
        Some(CapabilityError::Failed(FailureKind::Io)),
        "the untouched script lands on the next real call"
    );
}

// ---- D9/K5: sharing a fetched room file crosses as bytes, not an id ------

/// The whole inbound share path: the UI pumps `file.read` DATA into a
/// [`ShareSink`], commit materializes the bytes in the service's own custody,
/// and the resulting handle is what the share sheet accepts. No `(RoomId,
/// FileId)` and no URL ever reaches the platform service.
#[test]
fn a_fetched_room_file_is_shared_through_the_service_s_own_custody() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    let mut sink = block_on(services.files().share_sink(
        name("report.pdf"),
        Some(Mime::new("application/pdf")),
        &ct,
    ))
    .expect("share sink");
    block_on(sink.write(b"frag-1".to_vec())).expect("first chunk accepted");
    block_on(sink.write(b"frag-2".to_vec())).expect("second chunk accepted");
    let artifact = block_on(sink.commit()).expect("commit mints an artifact");
    assert_eq!(artifact.name().as_str(), "report.pdf");
    assert_eq!(artifact.size(), 12);

    let content = ShareContent {
        text: Some("look at this".to_owned()),
        attachment: Some(ShareAttachment::Fetched(artifact.clone())),
        anchor: None,
    };
    assert_eq!(
        settle(&controller, services.files().share_content(&content, &ct)),
        Ok(())
    );

    let effects = controller.effects();
    assert!(
        effects.iter().any(|e| matches!(
            e,
            RecordedEffect::StagedFetched { name, declared, bytes }
                if name == "report.pdf"
                    && declared.as_ref().map(Mime::as_str) == Some("application/pdf")
                    && bytes == b"frag-1frag-2"
        )),
        "the committed bytes reached the service's staging custody: {effects:?}"
    );
    assert!(
        effects.iter().any(|e| matches!(
            e,
            RecordedEffect::Shared { content, .. }
                if content.attachment == Some(ShareAttachment::Fetched(artifact.clone()))
        )),
        "the sheet was presented with the fetched artifact: {effects:?}"
    );
}

/// An artifact another service materialized is not shareable here — the same
/// minted-token gate blobs pass through, extended to the share sheet.
#[test]
fn a_fetched_artifact_from_another_service_fails_closed() {
    let (a, _a_ctl) = fake::android();
    let (b, b_ctl) = fake::android();
    let ct = CancelToken::new();
    let mut sink =
        block_on(a.files().share_sink(name("report.pdf"), None, &ct)).expect("share sink");
    block_on(sink.write(b"bytes".to_vec())).expect("chunk accepted");
    let artifact = block_on(sink.commit()).expect("commit");

    assert_eq!(
        block_on(b.files().share_content(
            &ShareContent::attachment(ShareAttachment::Fetched(artifact)),
            &ct
        )),
        Err(CapabilityError::Failed(FailureKind::Unreadable)),
        "service B never materialized this artifact"
    );
    assert!(
        b_ctl.open_dialogs().is_empty(),
        "a forged attachment never opens the share sheet"
    );
}

/// "Delete after share": a completed share consumes the artifact, so the same
/// handle cannot offer bytes the service no longer holds. A *dismissed* sheet
/// leaves it staged, so a retry needs no second download.
#[test]
fn a_shared_artifact_is_consumed_but_a_dismissal_keeps_it() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    let mut sink =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
    block_on(sink.write(b"payload".to_vec())).expect("chunk accepted");
    let artifact = block_on(sink.commit()).expect("commit");

    // A dismissal (scripted Cancelled) leaves the artifact staged.
    controller.force_error(Capability::ShareContent, CapabilityError::Cancelled);
    assert_eq!(
        settle(
            &controller,
            services.files().share_content(
                &ShareContent::attachment(ShareAttachment::Fetched(artifact.clone())),
                &ct
            )
        ),
        Err(CapabilityError::Cancelled)
    );

    // The retry succeeds — and consumes it.
    assert_eq!(
        settle(
            &controller,
            services.files().share_content(
                &ShareContent::attachment(ShareAttachment::Fetched(artifact.clone())),
                &ct
            )
        ),
        Ok(())
    );
    assert_eq!(
        settle(
            &controller,
            services.files().share_content(
                &ShareContent::attachment(ShareAttachment::Fetched(artifact)),
                &ct
            )
        ),
        Err(CapabilityError::Failed(FailureKind::Unreadable)),
        "a consumed artifact cannot be shared twice"
    );
}

/// Drop-is-abort (§D12/K2): an uncommitted share sink deletes its partial
/// artifact and mints no handle at all.
#[test]
fn dropping_an_uncommitted_share_sink_mints_nothing() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    {
        let mut sink =
            block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
        block_on(sink.write(b"partial".to_vec())).expect("chunk accepted");
        // Dropped without commit.
    }
    assert!(
        !controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::StagedFetched { .. })),
        "a dropped, uncommitted share sink records nothing"
    );
}

/// The share sink has its own scripted-failure path, surfaced where the sink is
/// created rather than swallowed mid-pump.
#[test]
fn share_sink_surfaces_a_forced_denial_at_creation() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    controller.force_error(Capability::ShareSink, CapabilityError::Denied);
    assert_eq!(
        block_on(services.files().share_sink(name("doc.bin"), None, &ct))
            .err()
            .expect("denied"),
        CapabilityError::Denied
    );
}

/// §K2 defence in depth: a token fired after the call but before the first poll
/// still yields `Cancelled`, never a live sink. The call-time gate cannot see
/// that window, and these three methods are the reference an M3–M5
/// implementation — where opening a SAF document or a download handle really
/// does suspend — will be written against.
#[test]
fn a_token_fired_before_the_first_poll_still_cancels() {
    let (services, controller) = fake::desktop();
    controller.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let ct = CancelToken::new();
    let target = settle(
        &controller,
        services.files().pick_export_target(name("out.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    block_on(async {
        let export = std::pin::pin!(services.files().export_sink(&target, &ct));
        let open = std::pin::pin!(services.files().open_sink(name("a.bin"), None, &ct));
        let share = std::pin::pin!(services.files().share_sink(name("b.bin"), None, &ct));
        // Fired after every call, before any of them is polled.
        ct.cancel();
        assert_eq!(export.await.err(), Some(CapabilityError::Cancelled));
        assert_eq!(open.await.err(), Some(CapabilityError::Cancelled));
        assert_eq!(share.await.err(), Some(CapabilityError::Cancelled));
    });
    assert!(
        !controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::ExportedLocal { .. })),
        "a cancelled sink writes nothing"
    );
}

/// Two shares of one artifact: the **first call** wins, whatever order the
/// executor polls them in.
///
/// The claim is taken at call time rather than at settle time precisely so this
/// is not a scheduling race — the fake's whole discipline is that release order
/// can never re-pair outcomes with calls. The second call finds the artifact
/// already claimed and fails without ever opening a sheet.
#[test]
fn overlapping_shares_are_claimed_in_call_order_not_poll_order() {
    for poll_second_first in [false, true] {
        let (services, controller) = fake::android();
        let ct = CancelToken::new();
        let mut sink =
            block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
        block_on(sink.write(b"payload".to_vec())).expect("chunk accepted");
        let artifact = block_on(sink.commit()).expect("commit");

        block_on(async {
            let first = std::pin::pin!(services.files().share_content(
                &ShareContent::attachment(ShareAttachment::Fetched(artifact.clone())),
                &ct
            ));
            let second = std::pin::pin!(services.files().share_content(
                &ShareContent::attachment(ShareAttachment::Fetched(artifact.clone())),
                &ct
            ));
            let mut first = first;
            let mut second = second;
            // The second call never opens a sheet: the artifact is already
            // claimed by the first.
            assert_eq!(
                controller.open_dialogs().len(),
                1,
                "only the first call opens a share sheet"
            );
            if poll_second_first {
                assert!(
                    matches!(
                        futures::poll!(second.as_mut()),
                        Poll::Ready(Err(CapabilityError::Failed(FailureKind::Unreadable)))
                    ),
                    "the second call loses regardless of poll order"
                );
            }
            assert!(futures::poll!(first.as_mut()).is_pending());
            assert!(controller.deliver_next());
            assert!(
                matches!(futures::poll!(first.as_mut()), Poll::Ready(Ok(()))),
                "the first call wins regardless of poll order"
            );
            if !poll_second_first {
                assert!(matches!(
                    futures::poll!(second.as_mut()),
                    Poll::Ready(Err(CapabilityError::Failed(FailureKind::Unreadable)))
                ));
            }
        });
        assert_eq!(
            controller
                .effects()
                .iter()
                .filter(|e| matches!(e, RecordedEffect::Shared { .. }))
                .count(),
            1,
            "exactly one share is recorded"
        );
        assert!(!controller.deliver_next(), "no stray sheet was left open");
    }
}

/// A share that does not complete must put the artifact back — dismissed,
/// scripted-failure, or simply dropped mid-flight — so a retry needs no second
/// download.
#[test]
fn an_incomplete_share_returns_the_artifact_to_staging() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    let mut sink =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
    block_on(sink.write(b"payload".to_vec())).expect("chunk accepted");
    let artifact = block_on(sink.commit()).expect("commit");
    let content = || ShareContent::attachment(ShareAttachment::Fetched(artifact.clone()));

    // Dropped mid-flight, before the sheet settles.
    block_on(async {
        let mut fut = std::pin::pin!(services.files().share_content(&content(), &ct));
        assert!(futures::poll!(fut.as_mut()).is_pending());
    });

    // Scripted failure.
    controller.force_error(Capability::ShareContent, CapabilityError::Denied);
    assert_eq!(
        settle(&controller, services.files().share_content(&content(), &ct)),
        Err(CapabilityError::Denied)
    );

    // Dismissed.
    controller.force_error(Capability::ShareContent, CapabilityError::Cancelled);
    assert_eq!(
        settle(&controller, services.files().share_content(&content(), &ct)),
        Err(CapabilityError::Cancelled)
    );

    // Still shareable after all three, and consumed only by the one that
    // completes.
    assert_eq!(
        settle(&controller, services.files().share_content(&content(), &ct)),
        Ok(())
    );
    assert_eq!(
        settle(&controller, services.files().share_content(&content(), &ct)),
        Err(CapabilityError::Failed(FailureKind::Unreadable)),
        "the completed share consumed it"
    );
}

// ---- D5: the outbound half of "delete after share" ----------------------

/// Only the UI knows whether the daemon's `file.share` settled, so the service
/// must be told when the staged bytes may go. Reaching reader EOF is not that
/// signal — a retry re-reads the same bytes — and dropping the opaque handle is
/// invisible to the service.
#[test]
fn releasing_a_staged_blob_reaps_it_and_is_final() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("doc.bin", None, b"0123456789".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let blob = settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");

    // Reading to EOF must NOT reap: the upload may still be retried.
    let mut reader = block_on(services.files().read_staged(&blob)).expect("reader");
    while block_on(reader.next_chunk(4))
        .expect("bounded pull")
        .is_some()
    {}
    drop(reader);
    assert!(
        block_on(services.files().read_staged(&blob)).is_ok(),
        "EOF is not a release — the bytes survive for a retry"
    );

    // A reader already open keeps working across the release, as an open fd
    // outlives an unlink.
    let mut live = block_on(services.files().read_staged(&blob)).expect("reader");
    assert_eq!(block_on(services.files().release_staged(&blob)), Ok(()));
    assert_eq!(
        block_on(live.next_chunk(4)).expect("bounded pull"),
        Some(b"0123".to_vec()),
        "an open reader survives the release"
    );

    assert!(
        controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::ReleasedStaged)),
        "the reap is recorded"
    );
    // Final: the bytes are gone and a second release fails closed.
    assert_eq!(
        block_on(services.files().read_staged(&blob)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable))
    );
    assert_eq!(
        block_on(services.files().release_staged(&blob)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "a release is final; a second one fails closed"
    );
}

/// A blob another service staged is not this service's to reap — the same
/// minted-token gate `read_staged` applies.
#[test]
fn releasing_another_services_blob_fails_closed() {
    let (a, a_ctl) = fake::desktop();
    let (b, _b_ctl) = fake::desktop();
    let ct = CancelToken::new();
    a_ctl.arm_pick("doc.bin", None, b"abcd".to_vec());
    let src = settle(&a_ctl, a.files().pick(&ct)).unwrap().unwrap();
    let blob = settle(
        &a_ctl,
        a.files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");
    assert_eq!(
        block_on(b.files().release_staged(&blob)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable))
    );
    // …and A's own release still works afterwards.
    assert_eq!(block_on(a.files().release_staged(&blob)), Ok(()));
}

// ---- §6: an operation that never starts consumes no script --------------

/// Validation precedes the script, not merely the sheet. A caller error must
/// leave a queued outcome for the next share that actually starts, instead of
/// eating it and masking the empty-content or anti-forgery failure.
#[test]
fn invalid_share_content_consumes_no_scripted_outcome() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.force_error(Capability::ShareContent, CapabilityError::Denied);

    let empty = ShareContent {
        text: None,
        attachment: None,
        anchor: None,
    };
    assert!(empty.is_empty());
    assert_eq!(
        block_on(services.files().share_content(&empty, &ct)),
        Err(CapabilityError::Failed(FailureKind::Io)),
        "empty content is a caller error, not the scripted denial"
    );
    assert!(controller.open_dialogs().is_empty(), "no sheet opened");

    // A forged attachment likewise reports its own failure.
    let (other, other_ctl) = fake::desktop();
    other_ctl.arm_pick("doc.bin", None, b"xy".to_vec());
    let src = settle(&other_ctl, other.files().pick(&ct))
        .unwrap()
        .unwrap();
    let foreign = settle(
        &other_ctl,
        other
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");
    assert_eq!(
        block_on(services.files().share_content(
            &ShareContent::attachment(ShareAttachment::Blob(foreign)),
            &ct
        )),
        Err(CapabilityError::Failed(FailureKind::Unreadable)),
        "a forged attachment reports the anti-forgery failure, not the script"
    );
    assert!(controller.open_dialogs().is_empty(), "no sheet opened");

    // The script is still queued for the share that actually starts.
    controller.arm_pick("own.bin", None, b"zz".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .unwrap()
        .unwrap();
    let mine = settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");
    assert_eq!(
        settle(
            &controller,
            services
                .files()
                .share_content(&ShareContent::attachment(ShareAttachment::Blob(mine)), &ct)
        ),
        Err(CapabilityError::Denied),
        "the untouched script lands on the first share that really starts"
    );
}

// ---- §6: a destination that fails mid-transfer --------------------------

/// A sink that fails partway through is a materially different path from one
/// that never opens: the caller must stop advancing CREDIT, abort, and drop the
/// artifact. It has to be scriptable, and the failed chunk must not be kept.
#[test]
fn a_sink_can_fail_mid_transfer_and_keeps_nothing() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let target = settle(
        &controller,
        services.files().pick_export_target(name("out.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    let mut sink = block_on(services.files().export_sink(&target, &ct)).expect("sink opens");
    block_on(sink.write(b"good".to_vec())).expect("first chunk accepted");
    // The disk fills on the second chunk.
    controller.force_error(
        Capability::FileSinkWrite,
        CapabilityError::Failed(FailureKind::Io),
    );
    assert_eq!(
        block_on(sink.write(b"lost".to_vec())).err(),
        Some(CapabilityError::Failed(FailureKind::Io)),
        "the destination failed mid-transfer"
    );
    drop(sink);
    assert!(
        !controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::ExportedLocal { .. })),
        "an aborted transfer publishes no artifact"
    );

    // The same path exists on the inbound staging sink.
    let mut share =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
    controller.force_error(
        Capability::ShareSinkWrite,
        CapabilityError::Failed(FailureKind::Io),
    );
    assert_eq!(
        block_on(share.write(b"lost".to_vec())).err(),
        Some(CapabilityError::Failed(FailureKind::Io))
    );
    drop(share);
    assert!(
        !controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::StagedFetched { .. })),
        "an aborted staging transfer mints no artifact"
    );
}

/// A destination that took every byte and then failed to **finalize** — a flush
/// error, a SAF document that would not close, a download that never published
/// — is its own path: the caller has nothing left to send and must still not
/// report success. It must be scriptable, and nothing may be published.
#[test]
fn a_sink_can_fail_at_finalization_and_publishes_nothing() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let target = settle(
        &controller,
        services.files().pick_export_target(name("out.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    let mut sink = block_on(services.files().export_sink(&target, &ct)).expect("sink opens");
    block_on(sink.write(b"every".to_vec())).expect("chunk accepted");
    block_on(sink.write(b"byte".to_vec())).expect("chunk accepted");
    controller.force_error(
        Capability::FileSinkCommit,
        CapabilityError::Failed(FailureKind::Io),
    );
    assert_eq!(
        block_on(sink.commit()).err(),
        Some(CapabilityError::Failed(FailureKind::Io)),
        "finalization failed after every byte was accepted"
    );
    assert!(
        !controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::ExportedLocal { .. })),
        "a failed commit publishes no artifact"
    );

    // The share sink's finalization mints no artifact, so nothing becomes
    // attachable.
    let mut share =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
    block_on(share.write(b"bytes".to_vec())).expect("chunk accepted");
    controller.force_error(
        Capability::ShareSinkCommit,
        CapabilityError::Failed(FailureKind::Io),
    );
    assert_eq!(
        block_on(share.commit()).err(),
        Some(CapabilityError::Failed(FailureKind::Io))
    );
    assert!(
        !controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::StagedFetched { .. })),
        "a failed staging commit mints no artifact"
    );
}

/// The **source** can fail after the reader is open — a staging file reaped, a
/// native read erroring partway through — which is the upload mirror of a sink
/// failing mid-download: the uploader must stop sending DATA and settle without
/// reaping bytes a retry still needs.
#[test]
fn a_staged_read_can_fail_after_the_reader_is_open() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("blob.bin", None, b"0123456789".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let blob = settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");
    let mut reader = block_on(services.files().read_staged(&blob)).expect("reader");
    assert_eq!(
        block_on(reader.next_chunk(4)).expect("first pull"),
        Some(b"0123".to_vec())
    );
    controller.force_error(
        Capability::StagedReadChunk,
        CapabilityError::Failed(FailureKind::Unreadable),
    );
    assert_eq!(
        block_on(reader.next_chunk(4)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "the source failed mid-upload"
    );
    // The failure consumed no position, and the bytes are still staged for a
    // retry — the upload must not reap them on a source error.
    assert_eq!(
        block_on(reader.next_chunk(4)).expect("the next pull"),
        Some(b"4567".to_vec())
    );
    assert!(block_on(services.files().read_staged(&blob)).is_ok());
}

// ---- D4: abandoned picker handles are releasable ------------------------

/// A pick the user abandons must be releasable: the service holds the real
/// `File`/path/`content://` grant behind an opaque token, and nothing else
/// tells it the object is dead. A `Drop` guard cannot do it — the handle is
/// `Clone`, so no copy knows it is the last.
#[test]
fn abandoned_picks_and_targets_can_be_discarded() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    controller.arm_pick("doc.bin", None, b"payload".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    assert_eq!(block_on(services.files().discard_source(&src)), Ok(()));
    assert!(
        controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::DiscardedSource)),
        "the release is recorded"
    );
    // Final, and the object is gone: staging it now fails closed.
    assert_eq!(
        block_on(services.files().discard_source(&src)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "a discard is final"
    );
    assert_eq!(
        block_on(
            services
                .files()
                .stage_for_share(&src, 1024, ProgressSink::discard(), &ct)
        )
        .err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "a discarded source cannot be staged"
    );

    controller.arm_export_target(ExportTargetKind::AndroidDocument, "out.bin");
    let target = settle(
        &controller,
        services.files().pick_export_target(name("out.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        block_on(services.files().discard_export_target(&target)),
        Ok(())
    );
    assert!(
        block_on(services.files().export_sink(&target, &ct)).is_err(),
        "a discarded target cannot be written to"
    );
}

/// Staging consumes the source, so the service does not keep the picked object
/// alive after it has been copied — and the same pick cannot be staged twice.
#[test]
fn staging_consumes_the_picked_source() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("doc.bin", None, b"payload".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let again = src.clone();
    assert!(settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct)
    )
    .is_ok());
    assert_eq!(
        block_on(
            services
                .files()
                .stage_for_share(&again, 1024, ProgressSink::discard(), &ct)
        )
        .err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "a staged source is consumed"
    );
    assert_eq!(
        block_on(services.files().discard_source(&again)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "and needs no discard"
    );
}

/// The source is consumed on **every** staging outcome, not just success: the
/// handle was moved in, so no caller can reach `discard_source` for it
/// afterwards, and a failed or cancelled stage would otherwise pin the file
/// object for the service's lifetime.
#[test]
fn staging_consumes_the_source_on_every_outcome() {
    for outcome in ["scripted", "cancelled"] {
        let (services, controller) = fake::desktop();
        let ct = CancelToken::new();
        controller.arm_pick("doc.bin", None, b"payload".to_vec());
        let src = settle(&controller, services.files().pick(&ct))
            .expect("pick ok")
            .expect("a source was picked");
        let again = src.clone();

        if outcome == "scripted" {
            controller.force_error(Capability::Stage, CapabilityError::Failed(FailureKind::Io));
            assert_eq!(
                block_on(services.files().stage_for_share(
                    &src,
                    1024,
                    ProgressSink::discard(),
                    &ct
                ))
                .err(),
                Some(CapabilityError::Failed(FailureKind::Io))
            );
        } else {
            ct.cancel();
            assert_eq!(
                block_on(services.files().stage_for_share(
                    &src,
                    1024,
                    ProgressSink::discard(),
                    &ct
                ))
                .err(),
                Some(CapabilityError::Cancelled)
            );
        }

        // The grant is gone either way: nothing is left to discard, and the
        // clone cannot be staged.
        assert_eq!(
            block_on(services.files().discard_source(&again)).err(),
            Some(CapabilityError::Failed(FailureKind::Unreadable)),
            "{outcome}: the source was already consumed"
        );
        let fresh = CancelToken::new();
        assert_eq!(
            block_on(services.files().stage_for_share(
                &again,
                1024,
                ProgressSink::discard(),
                &fresh
            ))
            .err(),
            Some(CapabilityError::Failed(FailureKind::Unreadable)),
            "{outcome}: a consumed source cannot be staged"
        );
    }
}

/// A fetched artifact the user backs out of must be releasable: committing a
/// share sink leaves real bytes in the service's staging area, and a successful
/// share is otherwise the only thing that removes them.
#[test]
fn an_abandoned_fetched_artifact_can_be_released() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    let mut sink =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
    block_on(sink.write(b"payload".to_vec())).expect("chunk accepted");
    let artifact = block_on(sink.commit()).expect("commit");

    // Backing out after a dismissed share must not strand the bytes.
    controller.force_error(Capability::ShareContent, CapabilityError::Cancelled);
    assert_eq!(
        settle(
            &controller,
            services.files().share_content(
                &ShareContent::attachment(ShareAttachment::Fetched(artifact.clone())),
                &ct
            )
        ),
        Err(CapabilityError::Cancelled)
    );
    assert_eq!(
        block_on(services.files().release_artifact(&artifact)),
        Ok(())
    );
    assert!(
        controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::ReleasedArtifact)),
        "the release is recorded"
    );
    // Final, and no longer shareable.
    assert_eq!(
        block_on(services.files().release_artifact(&artifact)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "a release is final"
    );
    assert_eq!(
        block_on(services.files().share_content(
            &ShareContent::attachment(ShareAttachment::Fetched(artifact)),
            &ct
        )),
        Err(CapabilityError::Failed(FailureKind::Unreadable))
    );
    assert!(controller.open_dialogs().is_empty());
}

/// A shared artifact is already consumed, so it needs no release.
#[test]
fn a_shared_artifact_needs_no_release() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    let mut sink =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
    block_on(sink.write(b"payload".to_vec())).expect("chunk accepted");
    let artifact = block_on(sink.commit()).expect("commit");
    assert_eq!(
        settle(
            &controller,
            services.files().share_content(
                &ShareContent::attachment(ShareAttachment::Fetched(artifact.clone())),
                &ct
            )
        ),
        Ok(())
    );
    assert_eq!(
        block_on(services.files().release_artifact(&artifact)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable))
    );
}

/// A picker that binds its result and then aborts must not strand the grant.
///
/// The caller never receives the handle, so `discard_source` is unreachable and
/// nothing else would ever reap it — which is invisible from the outside unless
/// the fake is asked what it is still holding. That is what
/// `retained_handles()` is for: without it this test passes whether or not the
/// grant leaks.
#[test]
fn an_aborted_picker_releases_its_bound_grant() {
    let (services, controller) = fake::android();
    assert_eq!(controller.retained_handles(), Default::default());

    // Cancelled after the call, before settlement.
    controller.arm_pick("doc.bin", None, b"payload".to_vec());
    let ct = CancelToken::new();
    block_on(async {
        let mut fut = std::pin::pin!(services.files().pick(&ct));
        assert!(futures::poll!(fut.as_mut()).is_pending());
        assert_eq!(
            controller.retained_handles().sources,
            1,
            "the armed source is bound and held while the dialog is open"
        );
        ct.cancel();
        assert_eq!(fut.await.err(), Some(CapabilityError::Cancelled));
    });
    assert_eq!(
        controller.retained_handles().sources,
        0,
        "a cancelled pick releases the file object it had bound"
    );

    // Dropped before settlement.
    controller.arm_pick("other.bin", None, b"payload".to_vec());
    let ct2 = CancelToken::new();
    block_on(async {
        let mut fut = std::pin::pin!(services.files().pick(&ct2));
        assert!(futures::poll!(fut.as_mut()).is_pending());
    });
    assert_eq!(
        controller.retained_handles().sources,
        0,
        "a dropped pick releases it too"
    );

    // The same for an export target's write grant.
    controller.arm_export_target(ExportTargetKind::AndroidDocument, "out.bin");
    let ct3 = CancelToken::new();
    block_on(async {
        let mut fut = std::pin::pin!(services.files().pick_export_target(name("out.bin"), &ct3));
        assert!(futures::poll!(fut.as_mut()).is_pending());
        assert_eq!(controller.retained_handles().export_targets, 1);
        ct3.cancel();
        assert_eq!(fut.await.err(), Some(CapabilityError::Cancelled));
    });
    assert_eq!(controller.retained_handles(), Default::default());

    // A pick that DOES reach the caller keeps its grant — the guard must not
    // over-reap.
    controller.arm_pick("kept.bin", None, b"payload".to_vec());
    let ct4 = CancelToken::new();
    let src = settle(&controller, services.files().pick(&ct4))
        .expect("pick ok")
        .expect("a source was picked");
    assert_eq!(
        controller.retained_handles().sources,
        1,
        "a delivered handle is the caller's, and still backed"
    );
    assert_eq!(block_on(services.files().discard_source(&src)), Ok(()));
    assert_eq!(controller.retained_handles(), Default::default());
}

/// `export_sink` takes its target by value, so every exit must drop the grant —
/// the caller cannot reach `discard_export_target` afterwards.
#[test]
fn export_sink_consumes_its_target_on_every_outcome() {
    for outcome in ["scripted", "cancelled"] {
        let (services, controller) = fake::android();
        let ct = CancelToken::new();
        controller.arm_export_target(ExportTargetKind::AndroidDocument, "out.bin");
        let target = settle(
            &controller,
            services.files().pick_export_target(name("out.bin"), &ct),
        )
        .unwrap()
        .unwrap();
        let again = target.clone();
        if outcome == "scripted" {
            controller.force_error(
                Capability::ExportSink,
                CapabilityError::Failed(FailureKind::Io),
            );
            assert!(block_on(services.files().export_sink(&target, &ct)).is_err());
        } else {
            ct.cancel();
            assert_eq!(
                block_on(services.files().export_sink(&target, &ct)).err(),
                Some(CapabilityError::Cancelled)
            );
        }
        assert_eq!(
            block_on(services.files().discard_export_target(&again)).err(),
            Some(CapabilityError::Failed(FailureKind::Unreadable)),
            "{outcome}: the grant was already consumed"
        );
        let fresh = CancelToken::new();
        assert!(
            block_on(services.files().export_sink(&again, &fresh)).is_err(),
            "{outcome}: a consumed target cannot be written to"
        );
    }
}

/// Releasing a staged blob while its sheet is open must not make the share a
/// lie: the operation holds the bytes for the sheet's lifetime, exactly as an
/// open reader outlives the unlink.
///
/// The recorded `attached_bytes` is read from that hold rather than looked up
/// again, which is what makes the hold observable: the blob is out of the
/// registry by settlement, so a size reported here can only have come from the
/// bytes this share kept alive.
#[test]
fn a_share_holds_its_staged_bytes_until_the_sheet_settles() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("doc.bin", None, b"payload".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let blob = settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");

    block_on(async {
        let mut share = std::pin::pin!(services.files().share_content(
            &ShareContent::attachment(ShareAttachment::Blob(blob.clone())),
            &ct
        ));
        assert!(
            futures::poll!(share.as_mut()).is_pending(),
            "the sheet is open"
        );
        // A clone is released underneath the still-open sheet.
        assert_eq!(services.files().release_staged(&blob).await, Ok(()));
        assert_eq!(
            controller.retained_handles().staged_blobs,
            1,
            "the registry entry is gone, but the open sheet still HOLDS the \
             bytes — reporting zero here would be the false clean bill"
        );
        assert!(controller.deliver_next());
        assert!(
            matches!(futures::poll!(share.as_mut()), Poll::Ready(Ok(()))),
            "the share held its bytes and stays honest"
        );
    });
    assert!(
        controller.effects().iter().any(|e| matches!(
            e,
            RecordedEffect::Shared {
                attached_bytes: Some(7),
                ..
            }
        )),
        "the settled share carried the real bytes, from its own hold: {:?}",
        controller.effects()
    );

    assert_eq!(
        controller.retained_handles().staged_blobs,
        0,
        "the hold ended with the sheet"
    );

    // …but the blob is gone for anything that starts afterwards.
    assert_eq!(
        block_on(services.files().read_staged(&blob)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable))
    );
    assert_eq!(
        block_on(
            services
                .files()
                .share_content(&ShareContent::attachment(ShareAttachment::Blob(blob)), &ct)
        ),
        Err(CapabilityError::Failed(FailureKind::Unreadable))
    );
}

/// Cleanup can itself fail — a locked temporary file, a permission change — and
/// the caller must be able to retry, so a failed release leaves the entry.
#[test]
fn a_failed_release_leaves_the_entry_recoverable() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    let mut sink =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
    block_on(sink.write(b"payload".to_vec())).expect("chunk accepted");
    let artifact = block_on(sink.commit()).expect("commit");

    controller.force_error(
        Capability::Release,
        CapabilityError::Failed(FailureKind::Io),
    );
    assert_eq!(
        block_on(services.files().release_artifact(&artifact)).err(),
        Some(CapabilityError::Failed(FailureKind::Io)),
        "the deletion failed"
    );
    assert!(
        !controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::ReleasedArtifact)),
        "a failed release reaps nothing"
    );
    // The retry succeeds, which is only possible because the entry survived.
    assert_eq!(
        block_on(services.files().release_artifact(&artifact)),
        Ok(())
    );
    assert_eq!(
        block_on(services.files().release_artifact(&artifact)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable))
    );
}

/// Cleanup outcomes bind at call time like every other scripted outcome: two
/// cleanup futures created in call order must keep their outcomes however the
/// executor polls them.
#[test]
fn cleanup_outcomes_bind_at_call_time() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    controller.arm_pick("doc.bin", None, b"payload".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let mut sink =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
    block_on(sink.write(b"payload".to_vec())).expect("chunk accepted");
    let artifact = block_on(sink.commit()).expect("commit");

    // One failure queued; the FIRST cleanup called must take it.
    controller.force_error(
        Capability::Release,
        CapabilityError::Failed(FailureKind::Io),
    );
    block_on(async {
        let mut first = std::pin::pin!(services.files().release_artifact(&artifact));
        let mut second = std::pin::pin!(services.files().discard_source(&src));
        // Polled in REVERSE order.
        assert!(
            matches!(futures::poll!(second.as_mut()), Poll::Ready(Ok(()))),
            "the second call is unaffected by the first call's scripted failure"
        );
        assert!(
            matches!(
                futures::poll!(first.as_mut()),
                Poll::Ready(Err(CapabilityError::Failed(FailureKind::Io)))
            ),
            "the first call keeps the failure it was scripted"
        );
    });
    // The failed release left its entry recoverable; the successful one reaped.
    assert_eq!(controller.retained_handles().share_artifacts, 1);
    assert_eq!(controller.retained_handles().sources, 0);
}

/// A window action can be refused by a platform that HAS windows — a compositor
/// declining a raise, an OS vetoing a close. §6 promises every capability
/// method's action outcome is forceable, and this one was not.
#[test]
fn window_actions_are_scriptable_where_windows_exist() {
    let (services, controller) = fake::desktop();
    assert!(services.window().availability().is_available());
    controller.force_error(Capability::WindowAction, CapabilityError::Denied);
    assert_eq!(
        services.window().request_close().err(),
        Some(CapabilityError::Denied),
        "a refused close is Denied, not a silent success"
    );
    assert!(
        controller.window_commands().is_empty(),
        "a refused command is not recorded as performed"
    );
    // The next command is unaffected, and is recorded.
    assert_eq!(services.window().minimize(), Ok(()));
    assert_eq!(
        controller.window_commands(),
        vec![jeliya_platform::WindowCommand::Minimize]
    );
    // Structural unavailability is still a shape fact, not a scripted one.
    let (browser, browser_ctl) = fake::browser();
    browser_ctl.force_error(Capability::WindowAction, CapabilityError::Denied);
    assert_eq!(
        browser.window().minimize().err(),
        Some(CapabilityError::Unavailable),
        "availability is decided before any script"
    );
}

/// A share that does not complete must leave the caller holding its attachment
/// handle — otherwise the service still owns the bytes and nothing can name
/// them. The content is borrowed, so this test deliberately never clones it.
#[test]
fn a_failed_share_leaves_the_caller_its_handle() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    let mut sink =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
    block_on(sink.write(b"payload".to_vec())).expect("chunk accepted");
    let artifact = block_on(sink.commit()).expect("commit");
    // The caller's ONE content value; a consuming API would take it here.
    let content = ShareContent::attachment(ShareAttachment::Fetched(artifact));

    controller.force_error(Capability::ShareContent, CapabilityError::Denied);
    assert_eq!(
        settle(&controller, services.files().share_content(&content, &ct)),
        Err(CapabilityError::Denied)
    );
    // Still usable: the same value retries, and the artifact was put back.
    assert_eq!(controller.retained_handles().share_artifacts, 1);
    assert_eq!(
        settle(&controller, services.files().share_content(&content, &ct)),
        Ok(()),
        "the caller could retry with the handle it still holds"
    );
    assert_eq!(controller.retained_handles().share_artifacts, 0);

    // A blob is not consumed by sharing at all, so the caller keeps it and can
    // still reap it afterwards.
    controller.arm_pick("blob.bin", None, b"bytes".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let blob = settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");
    let blob_content = ShareContent::attachment(ShareAttachment::Blob(blob.clone()));
    assert_eq!(
        settle(
            &controller,
            services.files().share_content(&blob_content, &ct)
        ),
        Ok(())
    );
    assert_eq!(
        block_on(services.files().release_staged(&blob)),
        Ok(()),
        "a shared blob is still the caller's to reap"
    );
    assert_eq!(controller.retained_handles(), Default::default());
}

/// §K2 through the whole transfer, not just its opening: a token fired after
/// the sink is returned must stop writes and prevent publication. Otherwise a
/// mid-copy cancellation becomes a published artifact, and an adapter that
/// keeps advancing CREDIT past a fired token looks correct against this fake.
#[test]
fn a_token_fired_mid_transfer_stops_the_sink() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let target = settle(
        &controller,
        services.files().pick_export_target(name("out.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    let mut sink = block_on(services.files().export_sink(&target, &ct)).expect("sink opens");
    block_on(sink.write(b"before".to_vec())).expect("accepted before cancellation");
    ct.cancel();
    assert_eq!(
        block_on(sink.write(b"after".to_vec())).err(),
        Some(CapabilityError::Cancelled),
        "a fired token stops the transfer"
    );
    assert_eq!(
        block_on(sink.commit()).err(),
        Some(CapabilityError::Cancelled),
        "and cancellation must never become a published artifact"
    );
    assert!(
        !controller
            .effects()
            .iter()
            .any(|e| matches!(e, RecordedEffect::ExportedLocal { .. })),
        "nothing was published"
    );

    // The same on the inbound staging sink.
    let ct2 = CancelToken::new();
    let mut share =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct2)).expect("share sink");
    block_on(share.write(b"before".to_vec())).expect("accepted");
    ct2.cancel();
    assert_eq!(
        block_on(share.write(b"after".to_vec())).err(),
        Some(CapabilityError::Cancelled)
    );
    assert_eq!(
        block_on(share.commit()).err(),
        Some(CapabilityError::Cancelled),
        "a cancelled staging transfer mints no artifact"
    );
    assert_eq!(controller.retained_handles().share_artifacts, 0);
}

/// A cleanup that fails must leave the caller its handle: the service keeps the
/// bytes precisely so the call can be retried, and a consuming signature would
/// make them unreachable. This test never clones.
#[test]
fn a_failed_cleanup_leaves_the_caller_its_handle() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    let mut sink =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
    block_on(sink.write(b"payload".to_vec())).expect("chunk accepted");
    let artifact = block_on(sink.commit()).expect("commit");

    controller.force_error(
        Capability::Release,
        CapabilityError::Failed(FailureKind::Io),
    );
    assert_eq!(
        block_on(services.files().release_artifact(&artifact)).err(),
        Some(CapabilityError::Failed(FailureKind::Io))
    );
    assert_eq!(controller.retained_handles().share_artifacts, 1);
    // The retry uses the same handle the caller still holds.
    assert_eq!(
        block_on(services.files().release_artifact(&artifact)),
        Ok(())
    );
    assert_eq!(controller.retained_handles(), Default::default());
}

/// Re-arming a picker replaces a handle no caller can ever receive, so the
/// entry it replaces must be reaped — otherwise setup code that revises an
/// armed result inflates the retained count for the fake's whole life.
#[test]
fn re_arming_a_picker_reaps_the_handle_it_replaces() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("first.bin", None, b"one".to_vec());
    controller.arm_pick("second.bin", None, b"two".to_vec());
    assert_eq!(
        controller.retained_handles().sources,
        1,
        "only the surviving armed source is held"
    );
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    assert_eq!(src.display_name().as_str(), "second.bin");
    assert_eq!(block_on(services.files().discard_source(&src)), Ok(()));
    assert_eq!(controller.retained_handles(), Default::default());

    controller.arm_export_target(ExportTargetKind::NativePath, "a.bin");
    controller.arm_export_target(ExportTargetKind::NativePath, "b.bin");
    assert_eq!(controller.retained_handles().export_targets, 1);
    let target = settle(
        &controller,
        services.files().pick_export_target(name("b.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        block_on(services.files().discard_export_target(&target)),
        Ok(())
    );
    assert_eq!(controller.retained_handles(), Default::default());
}

/// Opening a staged reader is its own failure moment — a permission that
/// changed before the upload began — distinct from a read failing partway
/// through, because nothing has been sent yet.
#[test]
fn opening_a_staged_reader_can_fail() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("blob.bin", None, b"0123456789".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let blob = settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");
    controller.force_error(
        Capability::ReadStaged,
        CapabilityError::Failed(FailureKind::Unreadable),
    );
    assert_eq!(
        block_on(services.files().read_staged(&blob)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "the staged file would not open"
    );
    // The bytes survive for a retry — an open failure reaps nothing.
    let mut reader = block_on(services.files().read_staged(&blob)).expect("the retry opens");
    assert_eq!(
        block_on(reader.next_chunk(4)).expect("pull"),
        Some(b"0123".to_vec())
    );
}

/// A zero bound is always a caller error, so it must be judged before the
/// script is dequeued — otherwise an invalid no-credit pull silently eats a
/// scripted source failure meant for the next real read.
#[test]
fn a_zero_bound_pull_consumes_no_scripted_failure() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("blob.bin", None, b"0123456789".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let blob = settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");
    let mut reader = block_on(services.files().read_staged(&blob)).expect("reader");
    controller.force_error(
        Capability::StagedReadChunk,
        CapabilityError::Failed(FailureKind::Unreadable),
    );
    assert_eq!(
        block_on(reader.next_chunk(0)).err(),
        Some(CapabilityError::Failed(FailureKind::Io)),
        "the zero bound reports its own error, not the scripted one"
    );
    assert_eq!(
        block_on(reader.next_chunk(4)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "the scripted source failure was left for the next real read"
    );
}

/// `retained_handles` must count bytes an in-flight share is holding. Reporting
/// zero while a pending sheet still owns a file would be exactly the false
/// clean bill a cleanup test would believe.
#[test]
fn in_flight_shares_count_as_retained() {
    let (services, controller) = fake::android();
    let ct = CancelToken::new();
    let mut sink =
        block_on(services.files().share_sink(name("doc.bin"), None, &ct)).expect("share sink");
    block_on(sink.write(b"payload".to_vec())).expect("chunk accepted");
    let artifact = block_on(sink.commit()).expect("commit");
    let content = ShareContent::attachment(ShareAttachment::Fetched(artifact));

    block_on(async {
        let mut share = std::pin::pin!(services.files().share_content(&content, &ct));
        assert!(
            futures::poll!(share.as_mut()).is_pending(),
            "the sheet is open"
        );
        // The claim took the bytes OUT of the registry — but the share owns
        // them, so they are still retained.
        assert_eq!(
            controller.retained_handles().share_artifacts,
            1,
            "an in-flight claim is still the service holding file bytes"
        );
        assert!(controller.deliver_next());
        assert!(matches!(
            futures::poll!(share.as_mut()),
            Poll::Ready(Ok(()))
        ));
    });
    assert_eq!(
        controller.retained_handles(),
        Default::default(),
        "the completed share consumed them"
    );
}

/// An invalid staged handle reports its own failure and consumes no scripted
/// open failure — the same validation-before-script rule the share sheet and
/// the zero-bound pull follow.
#[test]
fn an_invalid_staged_handle_consumes_no_open_failure() {
    let (a, a_ctl) = fake::desktop();
    let (b, b_ctl) = fake::desktop();
    let ct = CancelToken::new();
    // A blob belonging to another service.
    a_ctl.arm_pick("doc.bin", None, b"payload".to_vec());
    let src = settle(&a_ctl, a.files().pick(&ct)).unwrap().unwrap();
    let foreign = settle(
        &a_ctl,
        a.files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");

    b_ctl.force_error(
        Capability::ReadStaged,
        CapabilityError::Failed(FailureKind::Io),
    );
    assert_eq!(
        block_on(b.files().read_staged(&foreign)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "a foreign blob reports the anti-forgery failure, not the script"
    );

    // The script is still queued for the first VALID open.
    b_ctl.arm_pick("own.bin", None, b"mine".to_vec());
    let own_src = settle(&b_ctl, b.files().pick(&ct)).unwrap().unwrap();
    let own = settle(
        &b_ctl,
        b.files()
            .stage_for_share(&own_src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");
    assert_eq!(
        block_on(b.files().read_staged(&own)).err(),
        Some(CapabilityError::Failed(FailureKind::Io)),
        "the untouched script lands on the next valid open"
    );
}

/// Writing to an already-cancelled sink must not eat the scripted
/// partial-transfer failure: a buggy adapter that keeps writing would otherwise
/// make the next fresh sink unexpectedly succeed.
#[test]
fn a_pre_cancelled_sink_consumes_no_script() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let target = settle(
        &controller,
        services.files().pick_export_target(name("out.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    let mut sink = block_on(services.files().export_sink(&target, &ct)).expect("sink opens");
    controller.force_error(
        Capability::FileSinkWrite,
        CapabilityError::Failed(FailureKind::Io),
    );
    ct.cancel();
    // The adapter wrongly keeps writing after cancellation.
    assert_eq!(
        block_on(sink.write(b"after".to_vec())).err(),
        Some(CapabilityError::Cancelled),
        "cancellation wins, and the script is untouched"
    );
    assert_eq!(
        block_on(sink.commit()).err(),
        Some(CapabilityError::Cancelled)
    );

    // A fresh sink still meets the scripted failure.
    let fresh = CancelToken::new();
    controller.arm_export_target(ExportTargetKind::NativePath, "out2.bin");
    let target = settle(
        &controller,
        services
            .files()
            .pick_export_target(name("out2.bin"), &fresh),
    )
    .unwrap()
    .unwrap();
    let mut sink = block_on(services.files().export_sink(&target, &fresh)).expect("sink opens");
    assert_eq!(
        block_on(sink.write(b"chunk".to_vec())).err(),
        Some(CapabilityError::Failed(FailureKind::Io)),
        "the script was preserved for the sink that really writes"
    );
}

/// The progress contract: `transferred` counts bytes copied *so far*, reports
/// arrive one per bounded chunk, and a sink that cancels from the report gets
/// the figure for the chunk that had just landed, with the partial discarded.
///
/// Note on scope, because it matters for how much this test proves: the
/// *ordering* fix behind it — appending a chunk to the partial before
/// announcing it, so the figure is never ahead of the bytes — has **no
/// externally observable difference** in the fake. A cancelled stage records
/// nothing by design (§D5), so the partial is never surfaced to compare
/// against. This test therefore pins the progress contract itself, not the
/// internal ordering; the ordering matters because the M3–M5 implementations
/// read this fake as the reference for what `transferred` means.
#[test]
fn progress_reports_only_bytes_already_copied() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    // 10 bytes copied through an 8-byte buffer: two reports, 8 then 10.
    controller.arm_pick("doc.bin", None, b"0123456789".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = {
        let seen = Rc::clone(&seen);
        ProgressSink::new(move |p| seen.borrow_mut().push(p.transferred))
    };
    let blob = settle(
        &controller,
        services.files().stage_for_share(&src, 1024, sink, &ct),
    )
    .expect("stage ok");
    assert_eq!(*seen.borrow(), vec![8, 10]);
    assert_eq!(blob.size(), 10);

    // Cancelling from the report: the last figure announced is exactly what had
    // been copied, and the partial is discarded rather than recorded.
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("doc.bin", None, b"0123456789".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = {
        let seen = Rc::clone(&seen);
        let ct = ct.clone();
        ProgressSink::new(move |p| {
            seen.borrow_mut().push(p.transferred);
            ct.cancel();
        })
    };
    assert_eq!(
        block_on(services.files().stage_for_share(&src, 1024, sink, &ct)).err(),
        Some(CapabilityError::Cancelled)
    );
    assert_eq!(
        *seen.borrow(),
        vec![8],
        "the cancelling observer was told exactly what had been copied"
    );
    assert!(
        controller.staged().is_empty(),
        "a cancelled stage records nothing"
    );
}

/// A handle from another service fails closed *and* stays discardable on the
/// service that actually holds its grant. Consuming the caller's handle on that
/// path would strand the producing service's file object with nothing left to
/// name it — the reason every `Files` method borrows.
#[test]
fn a_foreign_handle_fails_closed_without_stranding_its_grant() {
    let (a, a_ctl) = fake::desktop();
    let (b, b_ctl) = fake::desktop();
    let ct = CancelToken::new();

    a_ctl.arm_pick("doc.bin", None, b"payload".to_vec());
    let src = settle(&a_ctl, a.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    assert_eq!(a_ctl.retained_handles().sources, 1);

    // The caller's ONE handle, offered to the wrong service.
    assert_eq!(
        block_on(
            b.files()
                .stage_for_share(&src, 1024, ProgressSink::discard(), &ct)
        )
        .err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable))
    );
    assert_eq!(
        a_ctl.retained_handles().sources,
        1,
        "A still holds the grant — and the caller still holds the handle"
    );
    assert_eq!(block_on(a.files().discard_source(&src)), Ok(()));
    assert_eq!(a_ctl.retained_handles(), Default::default());

    // The same for an export target's write grant.
    a_ctl.arm_export_target(ExportTargetKind::NativePath, "out.bin");
    let target = settle(&a_ctl, a.files().pick_export_target(name("out.bin"), &ct))
        .unwrap()
        .unwrap();
    assert!(block_on(b.files().export_sink(&target, &ct)).is_err());
    assert_eq!(a_ctl.retained_handles().export_targets, 1);
    assert_eq!(block_on(a.files().discard_export_target(&target)), Ok(()));
    assert_eq!(a_ctl.retained_handles(), Default::default());
    assert_eq!(b_ctl.retained_handles(), Default::default());
}

/// An invalid staging call reports its own failure and consumes no script, and
/// a known-oversize source fails before any copy — so neither steals the
/// outcome scripted for the next real staging call.
#[test]
fn invalid_staging_consumes_no_scripted_failure() {
    let (a, a_ctl) = fake::desktop();
    let (b, b_ctl) = fake::desktop();
    let ct = CancelToken::new();
    a_ctl.arm_pick("foreign.bin", None, b"payload".to_vec());
    let foreign = settle(&a_ctl, a.files().pick(&ct)).unwrap().unwrap();

    b_ctl.force_error(Capability::Stage, CapabilityError::Denied);
    assert_eq!(
        block_on(
            b.files()
                .stage_for_share(&foreign, 1024, ProgressSink::discard(), &ct)
        )
        .err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "a foreign source reports its own failure, not the script"
    );

    // A known-oversize source fails before any copy, and also takes no script.
    b_ctl.arm_pick("big.bin", None, vec![0u8; 64]);
    let big = settle(&b_ctl, b.files().pick(&ct)).unwrap().unwrap();
    assert!(matches!(
        block_on(
            b.files()
                .stage_for_share(&big, 8, ProgressSink::discard(), &ct)
        ),
        Err(CapabilityError::Failed(FailureKind::FileTooLarge {
            size: 64,
            limit: 8
        }))
    ));
    assert!(
        b_ctl.staged().is_empty(),
        "an oversize source is rejected before any copy"
    );

    // The script is still waiting for the first call that really stages.
    b_ctl.arm_pick("ok.bin", None, b"fits".to_vec());
    let ok = settle(&b_ctl, b.files().pick(&ct)).unwrap().unwrap();
    assert_eq!(
        block_on(
            b.files()
                .stage_for_share(&ok, 1024, ProgressSink::discard(), &ct)
        )
        .err(),
        Some(CapabilityError::Denied),
        "the untouched script lands on the next valid staging call"
    );
}

/// The same rule for `export_sink` and for cleanup: an invalid handle reports
/// its own failure and leaves the script for the next valid call.
#[test]
fn invalid_export_and_cleanup_consume_no_scripted_failure() {
    let (a, a_ctl) = fake::desktop();
    let (b, b_ctl) = fake::desktop();
    let ct = CancelToken::new();
    a_ctl.arm_export_target(ExportTargetKind::NativePath, "foreign.bin");
    let foreign = settle(
        &a_ctl,
        a.files().pick_export_target(name("foreign.bin"), &ct),
    )
    .unwrap()
    .unwrap();

    b_ctl.force_error(Capability::ExportSink, CapabilityError::Denied);
    assert_eq!(
        block_on(b.files().export_sink(&foreign, &ct)).err(),
        Some(CapabilityError::Failed(FailureKind::Io)),
        "a foreign target reports its own fail-closed Io, not the script"
    );
    b_ctl.arm_export_target(ExportTargetKind::NativePath, "own.bin");
    let own = settle(&b_ctl, b.files().pick_export_target(name("own.bin"), &ct))
        .unwrap()
        .unwrap();
    assert_eq!(
        block_on(b.files().export_sink(&own, &ct)).err(),
        Some(CapabilityError::Denied),
        "the untouched script lands on the next valid export sink"
    );

    // Cleanup: a foreign handle takes no Release script either.
    a_ctl.arm_pick("doc.bin", None, b"payload".to_vec());
    let src = settle(&a_ctl, a.files().pick(&ct)).unwrap().unwrap();
    b_ctl.force_error(
        Capability::Release,
        CapabilityError::Failed(FailureKind::Io),
    );
    assert_eq!(
        block_on(b.files().discard_source(&src)).err(),
        Some(CapabilityError::Failed(FailureKind::Unreadable)),
        "a foreign cleanup reports Unreadable, not the script"
    );
    b_ctl.arm_pick("own.bin", None, b"mine".to_vec());
    let own_src = settle(&b_ctl, b.files().pick(&ct)).unwrap().unwrap();
    assert_eq!(
        block_on(b.files().discard_source(&own_src)).err(),
        Some(CapabilityError::Failed(FailureKind::Io)),
        "the untouched script lands on the next valid cleanup"
    );
}

/// A known-empty source fails on its own terms before the script is touched —
/// the same call-time validation a known-oversize one gets, since both figures
/// are available before any copy.
#[test]
fn a_known_empty_source_consumes_no_scripted_failure() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.force_error(Capability::Stage, CapabilityError::Denied);
    controller.arm_pick("empty.bin", None, Vec::new());
    let empty = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    assert_eq!(
        block_on(
            services
                .files()
                .stage_for_share(&empty, 1024, ProgressSink::discard(), &ct)
        )
        .err(),
        Some(CapabilityError::Failed(FailureKind::FileEmpty)),
        "an empty source reports FileEmpty, not the script"
    );
    // The script is still queued for the first call that really stages.
    controller.arm_pick("ok.bin", None, b"fits".to_vec());
    let ok = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    assert_eq!(
        block_on(
            services
                .files()
                .stage_for_share(&ok, 1024, ProgressSink::discard(), &ct)
        )
        .err(),
        Some(CapabilityError::Denied),
        "the untouched script lands on the next valid staging call"
    );
}

/// An open reader deliberately outlives `release_staged`, so it is still the
/// service holding those bytes and must be counted as retained — the same
/// honesty in-flight share tracking was added for.
#[test]
fn an_open_staged_reader_counts_as_retained() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("blob.bin", None, b"0123456789".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let blob = settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");

    let mut reader = block_on(services.files().read_staged(&blob)).expect("reader");
    assert_eq!(block_on(services.files().release_staged(&blob)), Ok(()));
    assert_eq!(
        controller.retained_handles().staged_blobs,
        1,
        "the registry entry is gone, but the open reader still holds the bytes"
    );
    // …and it really does keep serving them.
    assert_eq!(
        block_on(reader.next_chunk(4)).expect("pull"),
        Some(b"0123".to_vec())
    );
    drop(reader);
    assert_eq!(
        controller.retained_handles(),
        Default::default(),
        "dropping the reader releases the last hold"
    );
}

/// The name a component *asks* for is observable independently of the target
/// the controller armed as the answer — otherwise a component that suggests the
/// wrong download name looks correct.
#[test]
fn the_requested_export_name_is_recorded() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_export_target(ExportTargetKind::NativePath, "armed.bin");
    let _ = settle(
        &controller,
        services
            .files()
            .pick_export_target(name("requested.bin"), &ct),
    )
    .unwrap()
    .unwrap();
    assert!(
        controller.effects().iter().any(|e| matches!(
            e,
            RecordedEffect::RequestedExportName { suggested } if suggested == "requested.bin"
        )),
        "the caller's suggestion is recorded, not just the armed answer: {:?}",
        controller.effects()
    );
}

/// An unpolled `read_staged` future already owns the bytes and can still yield
/// a working reader later, so it is already the service holding them.
#[test]
fn a_pending_staged_open_counts_as_retained() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("blob.bin", None, b"0123456789".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let blob = settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");

    block_on(async {
        // Created, never polled.
        let mut open = std::pin::pin!(services.files().read_staged(&blob));
        assert_eq!(services.files().release_staged(&blob).await, Ok(()));
        assert_eq!(
            controller.retained_handles().staged_blobs,
            1,
            "the pending open still owns the bytes"
        );
        // …and it really does still produce a working reader.
        let mut reader = match futures::poll!(open.as_mut()) {
            Poll::Ready(Ok(reader)) => reader,
            Poll::Ready(Err(error)) => panic!("the pending open still resolves: {error:?}"),
            Poll::Pending => panic!("the pending open still resolves"),
        };
        assert_eq!(
            reader.next_chunk(4).await.expect("pull"),
            Some(b"0123".to_vec())
        );
    });
    assert_eq!(
        controller.retained_handles(),
        Default::default(),
        "dropping the reader releases the last hold"
    );

    // A pending open that is never polled at all releases on drop.
    controller.arm_pick("second.bin", None, b"abcd".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");
    let blob = settle(
        &controller,
        services
            .files()
            .stage_for_share(&src, 1024, ProgressSink::discard(), &ct),
    )
    .expect("stage ok");
    block_on(async {
        let _open = services.files().read_staged(&blob);
        assert_eq!(controller.retained_handles().staged_blobs, 1);
    });
    assert_eq!(
        controller.retained_handles().staged_blobs,
        1,
        "still staged"
    );
    assert_eq!(block_on(services.files().release_staged(&blob)), Ok(()));
    assert_eq!(controller.retained_handles(), Default::default());
}

/// Two cleanups of the same handle are decided by CALL order, not by which
/// future the executor polls first.
#[test]
fn concurrent_cleanups_are_decided_in_call_order() {
    for poll_second_first in [false, true] {
        let (services, controller) = fake::desktop();
        let ct = CancelToken::new();
        controller.arm_pick("doc.bin", None, b"payload".to_vec());
        let src = settle(&controller, services.files().pick(&ct))
            .expect("pick ok")
            .expect("a source was picked");
        block_on(async {
            let mut first = std::pin::pin!(services.files().discard_source(&src));
            let mut second = std::pin::pin!(services.files().discard_source(&src));
            if poll_second_first {
                assert!(
                    matches!(
                        futures::poll!(second.as_mut()),
                        Poll::Ready(Err(CapabilityError::Failed(FailureKind::Unreadable)))
                    ),
                    "the second CALL loses regardless of poll order"
                );
            }
            assert!(
                matches!(futures::poll!(first.as_mut()), Poll::Ready(Ok(()))),
                "the first CALL wins regardless of poll order"
            );
            if !poll_second_first {
                assert!(matches!(
                    futures::poll!(second.as_mut()),
                    Poll::Ready(Err(CapabilityError::Failed(FailureKind::Unreadable)))
                ));
            }
        });
        assert_eq!(controller.retained_handles(), Default::default());
        assert_eq!(
            controller
                .effects()
                .iter()
                .filter(|e| matches!(e, RecordedEffect::DiscardedSource))
                .count(),
            1,
            "exactly one discard is recorded"
        );
    }
}

/// A cleanup that fails or is dropped puts the reserved entry back, so the
/// handle is exactly as it was and the caller can retry.
#[test]
fn a_reserved_cleanup_entry_is_restored_on_failure_or_drop() {
    let (services, controller) = fake::desktop();
    let ct = CancelToken::new();
    controller.arm_pick("doc.bin", None, b"payload".to_vec());
    let src = settle(&controller, services.files().pick(&ct))
        .expect("pick ok")
        .expect("a source was picked");

    // Dropped before it ever resolves.
    block_on(async {
        let _fut = services.files().discard_source(&src);
    });
    assert_eq!(
        controller.retained_handles().sources,
        1,
        "a dropped cleanup restores the entry"
    );

    // Scripted failure.
    controller.force_error(
        Capability::Release,
        CapabilityError::Failed(FailureKind::Io),
    );
    assert_eq!(
        block_on(services.files().discard_source(&src)).err(),
        Some(CapabilityError::Failed(FailureKind::Io))
    );
    assert_eq!(controller.retained_handles().sources, 1);

    // The retry succeeds — only possible because the entry came back.
    assert_eq!(block_on(services.files().discard_source(&src)), Ok(()));
    assert_eq!(controller.retained_handles(), Default::default());
}
