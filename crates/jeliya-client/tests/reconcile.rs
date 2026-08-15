//! Property and policy tests for the reconciler's public surface (#169 §8).
//!
//! These tests require no feature flags and touch only the public API exported
//! from `jeliya-client`. They verify structural and policy invariants — default
//! limits are finite and nonzero, every `ResyncReason` arm is constructible,
//! `ReconcileError` carries the room-limit information, and `RoomUpdate`
//! discriminants are accessible — without driving the async driver.

use jeliya_api::{GapReason, GapTo, RoomId};
use jeliya_client::{ReconcileError, ReconcileLimits, ResyncReason, ResyncRequired, RoomUpdate};

// -----------------------------------------------------------------------
// ReconcileLimits — defaults are finite and nonzero
// -----------------------------------------------------------------------

#[test]
fn reconcile_limits_default_buffer_depth_is_nonzero() {
    let limits = ReconcileLimits::default();
    assert!(limits.buffer_depth > 0, "buffer_depth must be positive");
}

#[test]
fn reconcile_limits_default_buffer_bytes_is_nonzero() {
    let limits = ReconcileLimits::default();
    assert!(limits.buffer_bytes > 0, "buffer_bytes must be positive");
}

#[test]
fn reconcile_limits_default_dedup_window_is_nonzero() {
    let limits = ReconcileLimits::default();
    assert!(limits.dedup_window > 0, "dedup_window must be positive");
}

#[test]
fn reconcile_limits_default_max_active_rooms_is_nonzero() {
    let limits = ReconcileLimits::default();
    assert!(
        limits.max_active_rooms > 0,
        "max_active_rooms must be positive"
    );
}

#[test]
fn reconcile_limits_default_read_page_size_is_nonzero() {
    let limits = ReconcileLimits::default();
    assert!(limits.read_page_size > 0, "read_page_size must be positive");
}

#[test]
fn reconcile_limits_default_external_input_byte_bounds_are_nonzero() {
    let limits = ReconcileLimits::default();
    assert!(limits.max_identifier_bytes > 0);
    assert!(limits.max_baseline_events > 0);
    assert!(limits.baseline_dedup_bytes > 0);
    assert!(limits.max_concurrent_reads > 0);
    assert!(limits.max_read_page_events > 0);
    assert!(limits.max_read_reply_bytes > 0);
    assert!(limits.max_read_reply_tokens > 0);
    assert!(limits.timeline_bytes > 0);
    assert!(limits.update_mailbox_bytes > 0);
}

#[test]
fn reconcile_limits_are_overridable_per_field() {
    let limits = ReconcileLimits {
        buffer_depth: 1,
        max_active_rooms: 1,
        ..ReconcileLimits::default()
    };
    assert_eq!(limits.buffer_depth, 1);
    assert_eq!(limits.max_active_rooms, 1);
    assert!(
        limits.buffer_bytes > 0,
        "un-overridden fields must retain the default"
    );
}

// -----------------------------------------------------------------------
// ResyncReason — every arm is constructible via the public type
// -----------------------------------------------------------------------

#[test]
fn resync_reason_bootstrap_is_constructible() {
    let _: ResyncReason = ResyncReason::Bootstrap;
}

#[test]
fn resync_reason_reconnect_is_constructible() {
    let _: ResyncReason = ResyncReason::Reconnect;
}

#[test]
fn resync_reason_gap_bounded_is_constructible() {
    let _: ResyncReason = ResyncReason::Gap {
        reason: GapReason::Retention,
        to: GapTo::Bounded { pos: 42 },
    };
}

#[test]
fn resync_reason_gap_open_is_constructible() {
    let _: ResyncReason = ResyncReason::Gap {
        reason: GapReason::SubscriptionLapse,
        to: GapTo::Open,
    };
}

#[test]
fn resync_reason_local_overflow_is_constructible() {
    let _: ResyncReason = ResyncReason::LocalOverflow { dropped: 7 };
}

#[test]
fn resync_reason_resync_required_by_daemon_is_constructible() {
    let _: ResyncReason = ResyncReason::ResyncRequiredByDaemon { from_pos: 100 };
}

#[test]
fn resync_reason_resume_is_constructible() {
    let _: ResyncReason = ResyncReason::Resume;
}

// -----------------------------------------------------------------------
// ResyncRequired — carries generation and reason
// -----------------------------------------------------------------------

#[test]
fn resync_required_fields_are_accessible() {
    let rr = ResyncRequired {
        generation: 3,
        reason: ResyncReason::Bootstrap,
    };
    assert_eq!(rr.generation, 3);
    assert!(matches!(rr.reason, ResyncReason::Bootstrap));
}

// -----------------------------------------------------------------------
// RoomUpdate — discriminants are accessible without driving the reconciler
// -----------------------------------------------------------------------

#[test]
fn room_update_resyncing_variant_is_accessible() {
    let room_id = RoomId::new("r");
    let update = RoomUpdate::Resyncing {
        room_id: room_id.clone(),
        resync: ResyncRequired {
            generation: 1,
            reason: ResyncReason::Bootstrap,
        },
    };
    assert!(matches!(update, RoomUpdate::Resyncing { .. }));
}

// -----------------------------------------------------------------------
// ReconcileError — TooManyRooms carries the configured limit
// -----------------------------------------------------------------------

#[test]
fn reconcile_error_too_many_rooms_names_the_limit() {
    let err = ReconcileError::TooManyRooms { limit: 16 };
    match err {
        ReconcileError::TooManyRooms { limit } => assert_eq!(limit, 16),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn reconcile_error_identifier_limit_is_accessible() {
    let err = ReconcileError::IdentifierTooLong { limit: 128 };
    assert!(matches!(
        err,
        ReconcileError::IdentifierTooLong { limit: 128 }
    ));
}

#[test]
fn reconcile_error_stopped_is_accessible() {
    let err = ReconcileError::Stopped;
    assert!(matches!(err, ReconcileError::Stopped));
}

#[test]
fn reconcile_error_display_mentions_limit() {
    let err = ReconcileError::TooManyRooms { limit: 32 };
    let msg = err.to_string();
    assert!(
        msg.contains("32"),
        "TooManyRooms display must mention the limit"
    );
}
