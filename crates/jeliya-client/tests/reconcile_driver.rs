//! Async-driver integration tests for the reconciler (#169 §7 step 10).
//!
//! These tests run the full `Reconciler::run()` loop against the deterministic
//! mock backend (`--features mock`) using `futures::executor::LocalPool` for
//! clock-free, single-threaded concurrent execution. Each
//! `pool.run_until_stalled()` drives all spawned tasks until no further
//! progress is possible without external input. Explicit
//! `controller.deliver_next()` and `controller.emit(event)` calls then provide
//! that input, giving full control over ordering with no wall-clock or
//! scheduler dependency.
//!
//! Coverage:
//! - AC-1 (Bootstrap): `Resyncing(Bootstrap)` observed before `Converged`.
//! - AC-1 (Reconnect): `Resyncing(Reconnect)` after lifecycle Interrupted→Ready.
//! - AC-6 (Resume): `Resyncing(Resume)` with no fabricated `StateChanged` on the seam.
//! - AC-3 (LocalOverflow): `stream.resync` rerun recovers the dropped event.
//! - Shutdown: `stop()` closes the view subscription stream cleanly.

use futures::executor::LocalPool;
use futures::task::LocalSpawnExt;
use futures::{FutureExt, StreamExt};
use jeliya_api::{
    Reachability, RoomId, RoomMembers, RoomMembersOut, RoomPeers, RoomPeersOut, RoomTimeline,
    RoomTimelineOut, StreamResync, StreamResyncOut, Truncated,
};
use jeliya_client::mock::{MockController, MockScript, Program};
use jeliya_client::{
    ClientEvent, ReconcileConfig, ReconcileLimits, Reconciler, ResyncReason, ResyncRequired,
    RoomPush, RoomUpdate, State,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal `jeliya_api::Event` via JSON deserialization — exactly the
/// same technique as `reconcile/core.rs` unit tests — so no `time` dev-dep is
/// needed.
fn make_event(pos: u64, id: &str) -> jeliya_api::Event {
    let json = format!(
        r#"{{"pos":{pos},"event_id":"{id}","at":"1970-01-01T00:00:00Z","author":{{"state":"unresolved"}},"kind":"message","content":{{"body":"x"}}}}"#
    );
    serde_json::from_str(&json).expect("test event JSON deserializes")
}

fn timeline_reply(room_id: &RoomId, events: Vec<jeliya_api::Event>) -> Program {
    Program::reply_ok::<RoomTimeline>(&RoomTimelineOut {
        room_id: room_id.clone(),
        events,
        truncated: Truncated::Complete,
    })
}

fn members_reply(room_id: &RoomId) -> Program {
    Program::reply_ok::<RoomMembers>(&RoomMembersOut {
        room_id: room_id.clone(),
        members: vec![],
    })
}

fn peers_reply(room_id: &RoomId) -> Program {
    Program::reply_ok::<RoomPeers>(&RoomPeersOut {
        room_id: room_id.clone(),
        reachability: Reachability::Offline,
        peers: vec![],
    })
}

fn resync_reply(room_id: &RoomId, events: Vec<jeliya_api::Event>, next_pos: u64) -> Program {
    Program::reply_ok::<StreamResync>(&StreamResyncOut {
        room_id: room_id.clone(),
        events,
        next_pos,
        truncated: Truncated::Complete,
    })
}

/// Drive three deliver+stall cycles for the bootstrap sequence
/// (room.timeline → room.members → room.peers). Completes when the
/// `Converged` view has been broadcast.
fn drive_bootstrap(pool: &mut LocalPool, ctrl: &MockController) {
    ctrl.deliver_next(); // deliver room.timeline
    pool.run_until_stalled(); // merges buffer, issues room.members
    ctrl.deliver_next(); // deliver room.members
    pool.run_until_stalled(); // issues room.peers
    ctrl.deliver_next(); // deliver room.peers
    pool.run_until_stalled(); // emits Converged, stalls
}

/// Drive three deliver+stall cycles for the incremental resync path
/// (stream.resync → room.members → room.peers), used for Reconnect, Resume,
/// and ResyncRequiredByDaemon (all of which `implicates_presence()`).
fn drive_resync_with_presence(pool: &mut LocalPool, ctrl: &MockController) {
    ctrl.deliver_next(); // deliver stream.resync
    pool.run_until_stalled(); // issues room.members
    ctrl.deliver_next(); // deliver room.members
    pool.run_until_stalled(); // issues room.peers
    ctrl.deliver_next(); // deliver room.peers
    pool.run_until_stalled(); // emits Converged, stalls
}

// ---------------------------------------------------------------------------
// AC-1 Bootstrap: Resyncing(Bootstrap) is observed before Converged
// ---------------------------------------------------------------------------

#[test]
fn bootstrap_emits_resyncing_then_converged() {
    let room_id = RoomId::new("room-bootstrap");

    let (handle, ctrl) = MockScript::new()
        .on("room.timeline", timeline_reply(&room_id, vec![]))
        .on("room.members", members_reply(&room_id))
        .on("room.peers", peers_reply(&room_id))
        .build();

    // Set state before spawning so run() reads Ready synchronously at start.
    ctrl.set_state(State::Ready);

    let reconciler = Reconciler::new(handle, ReconcileConfig::default());
    let mut sub = reconciler.subscribe();

    let mut pool = LocalPool::new();
    pool.spawner()
        .spawn_local({
            let r = reconciler.clone();
            async move { r.run().await }
        })
        .unwrap();

    // Activate: run() picks up the command, emits Resyncing{Bootstrap}, issues room.timeline.
    reconciler.activate_room(room_id.clone(), 0).unwrap();
    pool.run_until_stalled();

    // Read the Bootstrap notice while the timeline read is still in flight.
    let u1 = (&mut sub).next().now_or_never().flatten().unwrap();
    assert!(
        matches!(
            u1,
            RoomUpdate::Resyncing {
                resync: ResyncRequired {
                    reason: ResyncReason::Bootstrap,
                    ..
                },
                ..
            }
        ),
        "first update must be Resyncing(Bootstrap): {u1:?}"
    );

    drive_bootstrap(&mut pool, &ctrl);

    let u2 = (&mut sub).next().now_or_never().flatten().unwrap();
    assert!(
        matches!(u2, RoomUpdate::Converged(ref v) if v.room_id == room_id),
        "second update must be Converged: {u2:?}"
    );

    assert!(
        (&mut sub).next().now_or_never().is_none(),
        "no extra updates after converge"
    );
}

// ---------------------------------------------------------------------------
// AC-1 Reconnect: Resyncing(Reconnect) after lifecycle Interrupted → Ready
// ---------------------------------------------------------------------------

#[test]
fn reconnect_emits_resyncing_reconnect_then_converged() {
    let room_id = RoomId::new("room-reconnect");
    // Bootstrap must return at least one event so that the room's watermark is
    // set (Some(1)) after convergence. Without a watermark, begin_reconcile
    // treats Reconnect as non-incremental (watermark.is_some() == false) and
    // issues room.timeline again instead of stream.resync.
    let seed_event = make_event(1, "ev-seed-reconnect");

    let (handle, ctrl) = MockScript::new()
        // Bootstrap reads: timeline carries the seed event so watermark = Some(1).
        .on("room.timeline", timeline_reply(&room_id, vec![seed_event]))
        .on("room.members", members_reply(&room_id))
        .on("room.peers", peers_reply(&room_id))
        // Reconnect reads: incremental (watermark set) → stream.resync + members + peers.
        .on("stream.resync", resync_reply(&room_id, vec![], 1))
        .on("room.members", members_reply(&room_id))
        .on("room.peers", peers_reply(&room_id))
        .build();

    ctrl.set_state(State::Ready);

    let reconciler = Reconciler::new(handle, ReconcileConfig::default());

    let mut pool = LocalPool::new();
    pool.spawner()
        .spawn_local({
            let r = reconciler.clone();
            async move { r.run().await }
        })
        .unwrap();

    // Complete bootstrap (its notices are not the subject of this test).
    reconciler.activate_room(room_id.clone(), 0).unwrap();
    pool.run_until_stalled();
    drive_bootstrap(&mut pool, &ctrl);

    // Subscribe fresh so only reconnect updates are visible.
    let mut reconnect_sub = reconciler.subscribe();

    // Simulate a connection loss and recovery: Interrupted → Ready.
    ctrl.set_state(State::Interrupted);
    pool.run_until_stalled(); // core processes Interrupted (no reads in flight)
    ctrl.set_state(State::Ready);
    pool.run_until_stalled(); // core processes Ready → Resyncing(Reconnect) + stream.resync issued

    let u1 = (&mut reconnect_sub)
        .next()
        .now_or_never()
        .flatten()
        .unwrap();
    assert!(
        matches!(
            u1,
            RoomUpdate::Resyncing {
                resync: ResyncRequired {
                    reason: ResyncReason::Reconnect,
                    ..
                },
                ..
            }
        ),
        "first reconnect update must be Resyncing(Reconnect): {u1:?}"
    );

    drive_resync_with_presence(&mut pool, &ctrl);

    let u2 = (&mut reconnect_sub)
        .next()
        .now_or_never()
        .flatten()
        .unwrap();
    assert!(
        matches!(u2, RoomUpdate::Converged(ref v) if v.room_id == room_id),
        "second reconnect update must be Converged: {u2:?}"
    );
}

// ---------------------------------------------------------------------------
// AC-6 Resume: Resyncing(Resume) without a fabricated StateChanged on the seam
// ---------------------------------------------------------------------------

#[test]
fn resume_emits_resyncing_resume_without_fabricated_state_changed() {
    let room_id = RoomId::new("room-resume");
    // Seed event so watermark = Some(1) after bootstrap; Resume needs a set
    // watermark to take the incremental (stream.resync) path.
    let seed_event = make_event(1, "ev-seed-resume");

    let (handle, ctrl) = MockScript::new()
        .on("room.timeline", timeline_reply(&room_id, vec![seed_event]))
        .on("room.members", members_reply(&room_id))
        .on("room.peers", peers_reply(&room_id))
        // Resume reads: incremental (watermark set) → stream.resync + members + peers.
        .on("stream.resync", resync_reply(&room_id, vec![], 1))
        .on("room.members", members_reply(&room_id))
        .on("room.peers", peers_reply(&room_id))
        .build();

    ctrl.set_state(State::Ready);

    let reconciler = Reconciler::new(handle.clone(), ReconcileConfig::default());

    let mut pool = LocalPool::new();
    pool.spawner()
        .spawn_local({
            let r = reconciler.clone();
            async move { r.run().await }
        })
        .unwrap();

    // Bootstrap (discarded).
    reconciler.activate_room(room_id.clone(), 0).unwrap();
    pool.run_until_stalled();
    drive_bootstrap(&mut pool, &ctrl);

    // Subscribe to seam events AFTER bootstrap to detect any fabricated
    // StateChanged during the resume path (AC-6: resume must not pretend to reconnect).
    let mut seam_sub = handle.subscribe();
    let mut resume_sub = reconciler.subscribe();

    reconciler.resume().unwrap();
    pool.run_until_stalled(); // core processes Resume → Resyncing(Resume) + stream.resync issued

    // No StateChanged should have been injected on the seam event stream.
    assert!(
        seam_sub.next().now_or_never().is_none(),
        "resume must not fabricate any StateChanged on the seam (AC-6)"
    );

    let u1 = (&mut resume_sub).next().now_or_never().flatten().unwrap();
    assert!(
        matches!(
            u1,
            RoomUpdate::Resyncing {
                resync: ResyncRequired {
                    reason: ResyncReason::Resume,
                    ..
                },
                ..
            }
        ),
        "first resume update must be Resyncing(Resume): {u1:?}"
    );

    drive_resync_with_presence(&mut pool, &ctrl);

    let u2 = (&mut resume_sub).next().now_or_never().flatten().unwrap();
    assert!(
        matches!(u2, RoomUpdate::Converged(ref v) if v.room_id == room_id),
        "second resume update must be Converged: {u2:?}"
    );
}

// ---------------------------------------------------------------------------
// AC-3 LocalOverflow: stream.resync rerun recovers the event dropped from buffer
// ---------------------------------------------------------------------------

#[test]
fn overflow_rerun_recovers_dropped_event() {
    let room_id = RoomId::new("room-overflow");
    let e1 = make_event(1, "ev-1");
    let e2 = make_event(2, "ev-2");

    // buffer_depth=1: the first push fills the buffer; the second overflows.
    let config = ReconcileConfig {
        limits: ReconcileLimits {
            buffer_depth: 1,
            ..ReconcileLimits::default()
        },
    };

    let (handle, ctrl) = MockScript::new()
        // Bootstrap: timeline returns no history; e1 arrives as a live push
        // (buffered) and e2 overflows (dropped).
        .on("room.timeline", timeline_reply(&room_id, vec![]))
        .on("room.members", members_reply(&room_id))
        .on("room.peers", peers_reply(&room_id))
        // LocalOverflow recovery: stream.resync delivers e2.
        .on("stream.resync", resync_reply(&room_id, vec![e2.clone()], 2))
        .build();

    ctrl.set_state(State::Ready);

    let reconciler = Reconciler::new(handle, config);
    let mut sub = reconciler.subscribe();

    let mut pool = LocalPool::new();
    pool.spawner()
        .spawn_local({
            let r = reconciler.clone();
            async move { r.run().await }
        })
        .unwrap();

    // Activate: Resyncing{Bootstrap} emitted, room.timeline dispatched (pending).
    reconciler.activate_room(room_id.clone(), 0).unwrap();
    pool.run_until_stalled();

    // Read the Bootstrap notice before delivering any replies.
    let u1 = (&mut sub).next().now_or_never().flatten().unwrap();
    assert!(
        matches!(
            u1,
            RoomUpdate::Resyncing {
                resync: ResyncRequired {
                    reason: ResyncReason::Bootstrap,
                    ..
                },
                ..
            }
        ),
        "AC-3 setup: first update must be Resyncing(Bootstrap)"
    );

    // Emit two push events while the timeline read is in flight.
    // e1 fills the buffer (depth=1); e2 overflows → LocalOverflow queued internally.
    ctrl.emit(ClientEvent::Push(RoomPush::Event {
        room_id: room_id.clone(),
        event: e1.clone(),
    }));
    pool.run_until_stalled(); // core buffers e1 (buffer_depth now 1 = full)

    ctrl.emit(ClientEvent::Push(RoomPush::Event {
        room_id: room_id.clone(),
        event: e2.clone(),
    }));
    pool.run_until_stalled(); // core overflows e2 → LocalOverflow queued, e2 dropped

    // Complete bootstrap: timeline returns empty; core merges buffer [e1].
    // After peers: Converged(e1) + Resyncing{LocalOverflow} broadcast,
    // and stream.resync is issued as the queued rerun.
    drive_bootstrap(&mut pool, &ctrl);

    // Read both items queued by apply_actions while stream.resync is in flight.
    let u2 = (&mut sub).next().now_or_never().flatten().unwrap();
    assert!(
        matches!(u2, RoomUpdate::Converged(ref v) if v.timeline.len() == 1),
        "AC-3: first Converged must carry e1 only (e2 was dropped): {u2:?}"
    );

    let u3 = (&mut sub).next().now_or_never().flatten().unwrap();
    assert!(
        matches!(
            u3,
            RoomUpdate::Resyncing {
                resync: ResyncRequired {
                    reason: ResyncReason::LocalOverflow { .. },
                    ..
                },
                ..
            }
        ),
        "AC-3: LocalOverflow notice must follow the first Converged: {u3:?}"
    );

    // Deliver the resync that recovers e2.
    ctrl.deliver_next();
    pool.run_until_stalled(); // Converged(e1+e2) broadcast

    let u4 = (&mut sub).next().now_or_never().flatten().unwrap();
    assert!(
        matches!(u4, RoomUpdate::Converged(ref v) if v.timeline.len() == 2),
        "AC-3: recovery Converged must contain both events: {u4:?}"
    );
}

// ---------------------------------------------------------------------------
// Shutdown: stop() closes the view subscription stream cleanly
// ---------------------------------------------------------------------------

#[test]
fn stop_closes_subscription_and_ends_run() {
    let room_id = RoomId::new("room-stop");

    let (handle, _ctrl) = MockScript::new()
        // A hanging timeline so the read is in flight when we stop.
        .on("room.timeline", Program::hang(false))
        .build();

    _ctrl.set_state(State::Ready);

    let reconciler = Reconciler::new(handle, ReconcileConfig::default());
    let mut sub = reconciler.subscribe();

    let mut pool = LocalPool::new();
    pool.spawner()
        .spawn_local({
            let r = reconciler.clone();
            async move { r.run().await }
        })
        .unwrap();

    // Activate: Resyncing{Bootstrap} queued, room.timeline dispatched (hangs).
    reconciler.activate_room(room_id.clone(), 0).unwrap();
    pool.run_until_stalled();

    // Stop before the hanging read is delivered.
    reconciler.stop().unwrap();
    pool.run_until_stalled(); // run() processes Stop → cancels reads, closes view bus, returns

    // The Bootstrap Resyncing notice was queued before stop; it's still readable.
    let _ = (&mut sub).next().now_or_never().flatten();

    // Once drained, the stream must be closed (yields None, not Pending).
    let terminal = (&mut sub).next().now_or_never();
    assert!(
        matches!(terminal, Some(None)),
        "view subscription must close after stop(): {terminal:?}"
    );
}
