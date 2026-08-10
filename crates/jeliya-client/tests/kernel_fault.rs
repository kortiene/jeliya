//! Explicit fault coverage for the bounded client kernel (#168 §8 verification
//! bullets), driven by the deterministic in-memory transport. One test per
//! spec bullet. Requires feature `test-transport`; CI runs:
//!
//! ```text
//! cargo test -p jeliya-client --features test-transport
//! ```

use futures::executor::block_on;
use jeliya_api::{OpId, RoomCreate, RoomList};
use jeliya_client::{CallError, Dedup, Execution, KernelConfig, KernelLimits, State, TickDelta};
use jeliya_client::{ClientHandle, KernelController};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ready(limits: KernelLimits) -> (ClientHandle, KernelController) {
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig {
        limits,
        jitter_seed: 42,
        stable_principal: true,
    });
    handle.start();
    controller.connect();
    assert_eq!(handle.state(), State::Ready);
    (handle, controller)
}

fn default_ready() -> (ClientHandle, KernelController) {
    ready(KernelLimits::default())
}

// ---------------------------------------------------------------------------
// Stop-from-each-lifecycle-phase (Verification bullet: stop / §K11)
// ---------------------------------------------------------------------------

/// Verification (stop, Idle): stop before start transitions to Stopped with
/// nothing to settle and every collection empty (§K11).
#[test]
fn stop_from_idle_leaves_all_collections_empty() {
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig::default());
    block_on(handle.stop());
    assert_eq!(handle.state(), State::Stopped);
    assert_eq!(controller.outstanding(), 0);
    assert_eq!(controller.armed_timers(), 0);
}

/// Verification (stop, Connecting): stop while dialing settles every queued
/// call as `Cancelled { DefinitelyNot }` — nothing was sent past the gate
/// (§K11).
#[test]
fn stop_from_connecting_settles_queued_as_definitely_not() {
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig::default());
    handle.start(); // → Connecting, Dial
    assert_eq!(handle.state(), State::Connecting);

    let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
    assert_eq!(controller.queued(), 1);
    assert_eq!(
        controller.take_outbound().len(),
        0,
        "Connecting: no frames sent yet"
    );

    block_on(handle.stop());

    let err = block_on(fut).expect_err("the queued call is settled by stop");
    assert!(
        matches!(
            err,
            CallError::Cancelled {
                execution: Execution::DefinitelyNot
            }
        ),
        "never-sent call settles Cancelled{{DefinitelyNot}}, got {err:?}"
    );
    assert_eq!(handle.state(), State::Stopped);
    assert_eq!(controller.outstanding(), 0);
    assert_eq!(controller.armed_timers(), 0);
}

/// Verification (stop, mid-backoff): stop while in `Interrupted` state cancels
/// the backoff timer, settles queued calls as `Cancelled { DefinitelyNot }`,
/// and leaves every collection empty (§K11).
#[test]
fn stop_mid_backoff_cancels_timer_and_drains_queue() {
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig {
        limits: KernelLimits {
            max_reconnect_attempts: 8,
            ..KernelLimits::default()
        },
        jitter_seed: 42,
        stable_principal: true,
    });
    handle.start();
    controller.connect(); // → Ready (generation 1)
    controller.interrupt(); // → Interrupted (backoff timer armed)
    assert_eq!(handle.state(), State::Interrupted);

    assert!(
        controller.armed_timers() >= 1,
        "a backoff timer must be armed in Interrupted"
    );

    // A call dispatched while Interrupted queues (can't send without Ready).
    let queued_fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
    assert_eq!(controller.queued(), 1);

    block_on(handle.stop());

    let err = block_on(queued_fut).expect_err("the queued call is settled by stop");
    assert!(
        matches!(
            err,
            CallError::Cancelled {
                execution: Execution::DefinitelyNot
            }
        ),
        "never-sent call in Interrupted settles Cancelled{{DefinitelyNot}}, got {err:?}"
    );
    assert_eq!(handle.state(), State::Stopped);
    assert_eq!(controller.outstanding(), 0);
    assert_eq!(
        controller.armed_timers(),
        0,
        "stop cancels the backoff timer"
    );
}

/// Verification (stop, Ready with in-flight): stop while Ready with a sent
/// call settles it as `Cancelled { Unknown }` (bytes went out), settles a
/// queued call as `Cancelled { DefinitelyNot }`, and leaves every collection
/// empty (§K11, AC-5 execution-discriminant branch).
#[test]
fn stop_from_ready_with_in_flight_settles_all_and_empties_collections() {
    let (handle, controller) = ready(KernelLimits {
        in_flight: 1,
        queue_depth: 10,
        ..KernelLimits::default()
    });

    // First call: sent immediately (in-flight cap = 1).
    let sent_fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
    assert_eq!(controller.in_flight(), 1);
    let _frames = controller.take_outbound();

    // Second call: queued (in-flight cap reached).
    let queued_fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
    assert_eq!(controller.queued(), 1);

    block_on(handle.stop());

    let err_sent = block_on(sent_fut).expect_err("sent call settled by stop");
    let err_queued = block_on(queued_fut).expect_err("queued call settled by stop");

    assert!(
        matches!(
            err_sent,
            CallError::Cancelled {
                execution: Execution::Unknown
            }
        ),
        "sent call: Cancelled{{Unknown}}, got {err_sent:?}"
    );
    assert!(
        matches!(
            err_queued,
            CallError::Cancelled {
                execution: Execution::DefinitelyNot
            }
        ),
        "queued call: Cancelled{{DefinitelyNot}}, got {err_queued:?}"
    );
    assert_eq!(handle.state(), State::Stopped);
    assert_eq!(controller.outstanding(), 0, "ledger empty after stop");
    assert_eq!(controller.queued(), 0, "queue empty after stop");
    assert_eq!(controller.replay_held(), 0, "replay hold empty after stop");
    assert_eq!(
        controller.armed_timers(),
        0,
        "no orphaned timers after stop"
    );
}

/// Verification (stop, idempotent): a second `stop()` after the first is a
/// clean no-op; state remains `Stopped` (§K11).
#[test]
fn second_stop_is_idempotent() {
    let (handle, _controller) = default_ready();
    block_on(handle.stop());
    assert_eq!(handle.state(), State::Stopped);
    block_on(handle.stop()); // must not panic
    assert_eq!(handle.state(), State::Stopped);
}

// ---------------------------------------------------------------------------
// Gate-refused terminal path (Verification bullet: send/close races / §K7)
// ---------------------------------------------------------------------------

/// Verification (gate_refused): a terminal generation-gate refusal settles all
/// queued calls as `Disconnected { DefinitelyNot }` — nothing passed the
/// barrier — and transitions to `Failed` with no auto-retry. A subsequent
/// dispatch also gets `Cancelled { DefinitelyNot }` (§K7, AC-5 gate branch).
#[test]
fn gate_refused_settles_all_queued_as_definitely_not_and_blocks_future_calls() {
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig::default());
    handle.start(); // → Connecting

    let fut_a = handle.call::<RoomList>(RoomList {}, Dedup::None);
    let fut_b = handle.call::<RoomList>(RoomList {}, Dedup::None);
    assert_eq!(
        controller.take_outbound().len(),
        0,
        "no frames before the gate passes"
    );

    controller.gate_refused(); // terminal, no auto-retry

    let err_a = block_on(fut_a).expect_err("call A refused at gate");
    let err_b = block_on(fut_b).expect_err("call B refused at gate");
    assert!(
        matches!(
            err_a,
            CallError::Disconnected {
                execution: Execution::DefinitelyNot
            }
        ),
        "call A: {err_a:?}"
    );
    assert!(
        matches!(
            err_b,
            CallError::Disconnected {
                execution: Execution::DefinitelyNot
            }
        ),
        "call B: {err_b:?}"
    );
    assert_eq!(handle.state(), State::Failed);
    assert_eq!(controller.outstanding(), 0);

    // A call dispatched after terminal Failed must be refused immediately.
    let post_fail = handle.call::<RoomList>(RoomList {}, Dedup::None);
    let err_post = block_on(post_fail).expect_err("dispatch after Failed");
    assert!(
        matches!(
            err_post,
            CallError::Cancelled {
                execution: Execution::DefinitelyNot
            }
        ),
        "post-Failed dispatch: {err_post:?}"
    );
}

// ---------------------------------------------------------------------------
// Connected-after-stop ignored (Verification bullet: send/close races / §K11)
// ---------------------------------------------------------------------------

/// Verification (stop wins over Connected): a `Connected` signal that arrives
/// after stop must be ignored; state stays `Stopped` (§K11, stop-wins-over-
/// connect race).
#[test]
fn connected_after_stop_is_ignored() {
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig::default());
    block_on(handle.stop()); // stop from Idle → Stopped
    assert_eq!(handle.state(), State::Stopped);

    // A stale Connected arrives (e.g. dial completed just as stop fired).
    controller.connect();

    assert_eq!(
        handle.state(),
        State::Stopped,
        "a stale Connected must not revive the client after stop"
    );
    assert_eq!(controller.outstanding(), 0);
}

/// §K11 stop-wins: a gate refusal from a dial that `stop` already cancelled
/// must not flip the stopped client to `Failed` after the bus closed.
#[test]
fn gate_refusal_after_stop_is_ignored() {
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig::default());
    handle.start(); // → Connecting, a dial is in progress
    block_on(handle.stop()); // cancels the dial → Stopped
    assert_eq!(handle.state(), State::Stopped);

    // The refusal the cancelled dial already queued arrives late.
    controller.gate_refused();

    assert_eq!(
        handle.state(),
        State::Stopped,
        "stop wins: a late gate refusal cannot fail a stopped client"
    );
    assert_eq!(controller.outstanding(), 0);
}

/// §K14/§11 send/close race: the transport breaks exactly as the kernel
/// writes. The frame never reaches the wire, the loss surfaces as an
/// interruption, the replayable call is held, and the reconnect re-sends it.
#[test]
fn send_failure_at_flush_interrupts_and_replays() {
    let (handle, controller) = ready(KernelLimits {
        max_reconnect_attempts: 4,
        backoff_base: TickDelta::from_ticks(1),
        backoff_cap: TickDelta::from_ticks(1),
        ..KernelLimits::default()
    });
    controller.fail_send();
    let fut = handle.call::<RoomCreate>(
        RoomCreate {
            name: "send-race".into(),
        },
        Dedup::Key(OpId::new("op-send-race")),
    );
    assert_eq!(
        controller.take_outbound().len(),
        0,
        "the frame never reached the wire"
    );
    assert_eq!(handle.state(), State::Interrupted);
    assert_eq!(
        controller.replay_held(),
        1,
        "the sent-at-failure call is held"
    );

    // Reconnect: the held call re-sends and settles normally.
    controller.advance(1);
    controller.connect();
    let resent = controller.take_outbound();
    assert_eq!(resent.len(), 1, "re-sent on the new connection");
    controller.deliver_reply(resent[0].id, "{}");
    let _ = block_on(fut);
    assert_eq!(controller.outstanding(), 0);
}

/// §K14: a broken transport fails EVERY write in the batch — when one flush
/// emits several sends, no frame after the first failure reaches the wire,
/// so the driver never falsely claims later requests were sent.
#[test]
fn send_failure_drops_every_frame_in_the_batch() {
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig {
        limits: KernelLimits {
            max_reconnect_attempts: 4,
            backoff_base: TickDelta::from_ticks(1),
            backoff_cap: TickDelta::from_ticks(1),
            ..KernelLimits::default()
        },
        jitter_seed: 42,
        stable_principal: true,
    });
    handle.start(); // Connecting: dispatches queue, nothing sends
    let fut_a = handle.call::<RoomCreate>(
        RoomCreate { name: "a".into() },
        Dedup::Key(OpId::new("op-batch-a")),
    );
    let fut_b = handle.call::<RoomCreate>(
        RoomCreate { name: "b".into() },
        Dedup::Key(OpId::new("op-batch-b")),
    );
    controller.fail_send();
    // Connected flushes both queued calls in ONE batch; the first write
    // breaks the pipe, so the second must not reach the wire either.
    controller.connect();
    assert_eq!(
        controller.take_outbound().len(),
        0,
        "no frame in the failed batch reaches the wire"
    );
    assert_eq!(handle.state(), State::Interrupted);
    assert_eq!(
        controller.replay_held(),
        2,
        "both sent-at-failure calls are held for replay"
    );
    // The reconnect re-sends both and they settle normally.
    controller.advance(1);
    controller.connect();
    let resent = controller.take_outbound();
    assert_eq!(resent.len(), 2, "both held calls re-send");
    for frame in &resent {
        controller.deliver_reply(frame.id, "{}");
    }
    let _ = block_on(fut_a);
    let _ = block_on(fut_b);
    assert_eq!(controller.outstanding(), 0);
}

/// §K5: without the stable-principal certification (the safe default),
/// nothing auto-replays — a disconnect settles a keyed mutation honestly as
/// `Disconnected { Unknown }` instead of re-executing it under a fresh
/// ephemeral principal whose dedup ledger never saw the op_id.
#[test]
fn an_ephemeral_principal_never_auto_replays() {
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig {
        limits: KernelLimits {
            backoff_base: TickDelta::from_ticks(1),
            backoff_cap: TickDelta::from_ticks(1),
            ..KernelLimits::default()
        },
        jitter_seed: 42,
        stable_principal: false,
    });
    handle.start();
    controller.connect();
    let fut = handle.call::<RoomCreate>(
        RoomCreate {
            name: "ephemeral".into(),
        },
        Dedup::Key(OpId::new("op-ephemeral")),
    );
    assert_eq!(controller.take_outbound().len(), 1);
    controller.interrupt();
    assert_eq!(
        controller.replay_held(),
        0,
        "nothing is held without the principal certification"
    );
    let err = block_on(fut).expect_err("settled by the disconnect");
    assert!(
        matches!(
            err,
            CallError::Disconnected {
                execution: Execution::Unknown
            }
        ),
        "honest Unknown, no auto-replay: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// In-flight throttle invariant (AC-1 / AC-7)
// ---------------------------------------------------------------------------

/// AC-1 / AC-7: the kernel never puts more than `in_flight` calls on the wire
/// simultaneously; calls beyond the cap queue and flush as replies land. The
/// `in_flight_count <= in_flight` invariant holds across the full drain cycle.
#[test]
fn in_flight_throttle_never_exceeds_configured_limit() {
    const LIMIT: u32 = 2;
    let (handle, controller) = ready(KernelLimits {
        in_flight: LIMIT,
        queue_depth: 20,
        ..KernelLimits::default()
    });

    // Dispatch 5 calls; with in_flight=2, exactly LIMIT go out immediately.
    let futs: Vec<_> = (0..5)
        .map(|_| handle.call::<RoomList>(RoomList {}, Dedup::None))
        .collect();

    assert_eq!(
        controller.in_flight(),
        LIMIT,
        "exactly {LIMIT} on the wire after dispatch"
    );
    assert_eq!(
        controller.queued(),
        5 - LIMIT as usize,
        "remainder queued behind throttle"
    );

    // Reply to all calls; each reply flushes the next queued call onto the wire.
    // Assert the throttle invariant at the start of every batch.
    let mut replied = 0_usize;
    while replied < 5 {
        assert!(
            controller.in_flight() <= LIMIT,
            "throttle violated: in_flight={} > {LIMIT} (replied={replied})",
            controller.in_flight()
        );
        let batch = controller.take_outbound();
        assert!(
            !batch.is_empty(),
            "expected in-flight frames, got none (replied={replied})"
        );
        for frame in batch {
            controller.deliver_reply(frame.id, "{\"rooms\":[]}");
            replied += 1;
        }
    }

    for fut in futs {
        let _ = block_on(fut).expect("all 5 calls resolve successfully");
    }
    assert_eq!(controller.outstanding(), 0, "ledger fully drained");
}

// ---------------------------------------------------------------------------
// Timeout while in-flight (Verification bullet: timeout / §K8)
// ---------------------------------------------------------------------------

/// Verification (timeout, in-flight): a sent call whose deadline fires settles
/// `Timeout` (may still land → `Unknown` is the implication); a subsequent
/// real reply for that wire id is absorbed by the tombstone and strands nothing
/// (§K8).
#[test]
fn timeout_while_in_flight_settles_timeout_and_absorbs_late_reply() {
    const DEADLINE: u64 = 10;
    let (handle, controller) = ready(KernelLimits {
        default_call_deadline: TickDelta::from_ticks(DEADLINE),
        ..KernelLimits::default()
    });

    let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
    let sent = controller.take_outbound();
    assert_eq!(sent.len(), 1, "call reached the wire");
    let wire_id = sent[0].id;

    // Advance the virtual clock past the deadline to fire the per-call timer.
    controller.advance(DEADLINE);

    let err = block_on(fut).expect_err("timed-out call settles as an error");
    assert!(
        matches!(err, CallError::Timeout),
        "expected Timeout, got {err:?}"
    );

    // A late real reply for the timed-out id is absorbed by the tombstone —
    // no strand, no double-settle, no panic.
    controller.deliver_reply(wire_id, "{\"rooms\":[]}");
    assert_eq!(
        controller.outstanding(),
        0,
        "tombstone reclaimed after absorbing late reply"
    );
}

// ---------------------------------------------------------------------------
// Timeout while queued (Verification bullet: timeout / §K8)
// ---------------------------------------------------------------------------

/// Verification (timeout, queued): a call admitted to the bounded queue whose
/// deadline fires before it is ever sent settles `Timeout` and releases its
/// admission charge so the queue immediately accepts a replacement (§K8, §K2).
#[test]
fn timeout_while_queued_releases_admission_and_allows_readmission() {
    const DEADLINE: u64 = 10;
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig {
        limits: KernelLimits {
            queue_depth: 1, // tight: confirms the charge is released
            default_call_deadline: TickDelta::from_ticks(DEADLINE),
            ..KernelLimits::default()
        },
        jitter_seed: 42,
        stable_principal: true,
    });
    // Idle: the call is admitted to the queue but never sent.
    let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
    assert_eq!(controller.queued(), 1);
    assert_eq!(
        controller.take_outbound().len(),
        0,
        "Idle: nothing sent before the gate"
    );

    // Fire the deadline by advancing past it.
    controller.advance(DEADLINE);
    let err = block_on(fut).expect_err("queued call times out before being sent");
    assert!(
        matches!(err, CallError::Timeout),
        "expected Timeout, got {err:?}"
    );
    assert_eq!(controller.outstanding(), 0, "ledger empty");
    assert_eq!(controller.queued(), 0, "queue empty");

    // The admission charge is released: the depth-1 queue accepts a fresh call.
    let fut2 = handle.call::<RoomList>(RoomList {}, Dedup::None);
    assert_eq!(
        controller.queued(),
        1,
        "fresh call admitted after charge released"
    );
    drop(fut2);
}

// ---------------------------------------------------------------------------
// outbound_bytes QueueFull (Verification bullet: queue saturation / §K2)
// ---------------------------------------------------------------------------

/// Verification (queue saturation, outbound_bytes): exceeding the byte cap
/// surfaces `QueueFull { outbound_bytes }` through the typed handle — visible,
/// never absorbed (§K2, AC-1).
///
/// `RoomList {}` serialises to `{}` (2 bytes). `outbound_bytes: 2` fits exactly
/// one call; the second overflows.
#[test]
fn outbound_bytes_queue_full_surfaces_through_handle() {
    let (handle, _controller) = ClientHandle::with_kernel(KernelConfig {
        limits: KernelLimits {
            outbound_bytes: 2, // one `{}` payload fits; a second overflows
            ..KernelLimits::default()
        },
        jitter_seed: 42,
        stable_principal: true,
    });
    let _first = handle.call::<RoomList>(RoomList {}, Dedup::None);
    let second = handle.call::<RoomList>(RoomList {}, Dedup::None);
    let err = block_on(second).expect_err("second call overflows outbound_bytes");
    assert!(
        matches!(
            err,
            CallError::QueueFull {
                resource: "outbound_bytes",
                limit: 2
            }
        ),
        "expected QueueFull{{outbound_bytes=2}}, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Connection loss: never-sent vs may-have-executed (AC-4 / §K6)
// ---------------------------------------------------------------------------

/// AC-4: a recoverable connection loss that immediately exhausts the reconnect
/// budget distinguishes queued calls (`Disconnected { DefinitelyNot }` — never
/// on the wire) from sent calls (`Disconnected { Unknown }` — bytes went out)
/// (§K6, AC-4).
#[test]
fn interrupted_classifies_never_sent_as_definitely_not_and_sent_as_unknown() {
    let (handle, controller) = ready(KernelLimits {
        in_flight: 1,
        max_reconnect_attempts: 0, // any loss immediately exhausts the budget
        ..KernelLimits::default()
    });

    // First call goes on the wire; second is throttled into the queue.
    let sent_fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
    assert_eq!(controller.in_flight(), 1);
    let _frames = controller.take_outbound();

    let queued_fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
    assert_eq!(controller.queued(), 1);

    // A single interrupt exhausts the zero-attempt budget → Failed immediately.
    controller.interrupt();
    assert_eq!(handle.state(), State::Failed);

    let err_sent = block_on(sent_fut).expect_err("sent call settled on disconnect");
    let err_queued = block_on(queued_fut).expect_err("queued call settled on disconnect");

    assert!(
        matches!(
            err_sent,
            CallError::Disconnected {
                execution: Execution::Unknown
            }
        ),
        "sent call: Disconnected{{Unknown}}, got {err_sent:?}"
    );
    assert!(
        matches!(
            err_queued,
            CallError::Disconnected {
                execution: Execution::DefinitelyNot
            }
        ),
        "queued call: Disconnected{{DefinitelyNot}}, got {err_queued:?}"
    );
    assert_eq!(controller.outstanding(), 0, "ledger fully drained");
}

// ---------------------------------------------------------------------------
// Malformed frame (Verification bullet: decoder failure / §K4)
// ---------------------------------------------------------------------------

/// Verification (decoder failure, malformed): a malformed inbound frame (one
/// that parses to no envelope) is dropped silently — it strands no call and
/// cannot double-settle or mis-route anything (§K4).
#[test]
fn malformed_frame_does_not_strand_outstanding_call() {
    let (handle, controller) = default_ready();

    let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
    let sent = controller.take_outbound();
    assert_eq!(sent.len(), 1);
    let wire_id = sent[0].id;
    assert_eq!(controller.outstanding(), 1);

    // The malformed frame is silently dropped — the outstanding call is untouched.
    controller.deliver_malformed();
    assert_eq!(
        controller.outstanding(),
        1,
        "outstanding call untouched by a malformed frame"
    );

    // The real reply for the same wire id still settles the call.
    controller.deliver_reply(wire_id, "{\"rooms\":[]}");
    let _ = block_on(fut).expect("real reply settles the call");
    assert_eq!(
        controller.outstanding(),
        0,
        "fully settled after real reply"
    );
}

// ---------------------------------------------------------------------------
// Reconnect exhaustion (Verification bullet: reconnect exhaustion / §K10)
// ---------------------------------------------------------------------------

/// Verification (reconnect exhaustion): after `max_reconnect_attempts`
/// consecutive failures the kernel transitions to `Failed` and settles every
/// outstanding call honestly — queued (never-sent) calls as
/// `Disconnected { DefinitelyNot }` — without spinning forever (§K10, AC-7).
///
/// `max_reconnect_attempts: 2` requires three `interrupt()` calls to exhaust
/// (attempt 0, 1, then 2 >= 2 fails). A clock advance between each fires the
/// backoff timer so the core sees a genuine reconnect-failure cycle.
#[test]
fn reconnect_exhaustion_fails_and_settles_outstanding_calls() {
    let (handle, controller) = ClientHandle::with_kernel(KernelConfig {
        limits: KernelLimits {
            max_reconnect_attempts: 2,
            backoff_base: TickDelta::from_ticks(1),
            backoff_cap: TickDelta::from_ticks(1), // fire within 1 tick
            ..KernelLimits::default()
        },
        jitter_seed: 42,
        stable_principal: true,
    });
    handle.start();
    assert_eq!(handle.state(), State::Connecting);

    // A call dispatched while Connecting queues behind the gate.
    let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
    assert_eq!(
        controller.take_outbound().len(),
        0,
        "Connecting: nothing sent"
    );
    assert_eq!(controller.queued(), 1);

    // Three interrupts (with a clock tick between each to fire the backoff timer)
    // exhaust the two-attempt budget and land the kernel in Failed.
    controller.interrupt(); // attempt 0 → 1, Interrupted
    assert_eq!(handle.state(), State::Interrupted);
    controller.advance(1); // fire backoff timer → Dial

    controller.interrupt(); // attempt 1 → 2, Interrupted
    assert_eq!(handle.state(), State::Interrupted);
    controller.advance(1); // fire backoff timer → Dial

    controller.interrupt(); // attempt 2 >= max 2 → fail_all → Failed
    assert_eq!(handle.state(), State::Failed);

    // The queued call (never sent) settles DefinitelyNot.
    let err = block_on(fut).expect_err("queued call settled by exhaustion");
    assert!(
        matches!(
            err,
            CallError::Disconnected {
                execution: Execution::DefinitelyNot
            }
        ),
        "never-sent queued call: Disconnected{{DefinitelyNot}}, got {err:?}"
    );
    assert_eq!(controller.outstanding(), 0, "ledger empty after exhaustion");
    assert_eq!(
        controller.armed_timers(),
        0,
        "no orphaned timers after exhaustion"
    );
}

// ---------------------------------------------------------------------------
// Generation fencing: stale reply fenced, fresh reply settles (AC-6 / §K7)
// ---------------------------------------------------------------------------

/// AC-6 / Verification (generation fencing): a stale-generation reply (old
/// wire id, old generation) for a replayable call that was re-sent on the new
/// connection is fenced and dropped; only the fresh-generation reply settles
/// the call exactly once (§K7, AC-6).
///
/// Uses `RoomCreate + Dedup::Key` to get `ReplayableUnderOpId` so the call is
/// held across the reconnect and re-sent under a new wire id and generation.
#[test]
fn generation_fencing_stale_reply_is_fenced_fresh_reply_settles() {
    let (handle, controller) = ready(KernelLimits {
        max_reconnect_attempts: 4,
        backoff_base: TickDelta::from_ticks(1),
        backoff_cap: TickDelta::from_ticks(1),
        ..KernelLimits::default()
    });

    // A mutating + op_id call earns ReplayableUnderOpId (§K5).
    let fut = handle.call::<RoomCreate>(
        RoomCreate {
            name: "fence-test".into(),
        },
        Dedup::Key(OpId::new("op-gen-fence")),
    );

    let sent = controller.take_outbound();
    assert_eq!(sent.len(), 1, "call sent on generation 1");
    let wire_id_gen1 = sent[0].id;
    let gen1 = controller.generation();

    // Lose the connection: replayable call is held, not settled.
    controller.interrupt();
    assert_eq!(
        controller.replay_held(),
        1,
        "replayable call held for replay"
    );
    assert_eq!(controller.outstanding(), 1, "call still in the ledger");

    // Fire the backoff timer and reconnect: the held call is re-sent under gen2.
    controller.advance(1);
    let gen2 = controller.connect();
    assert_eq!(gen2, gen1 + 1, "generation incremented on reconnect");
    assert_eq!(
        controller.replay_held(),
        0,
        "held call flushed on reconnect"
    );

    let resent = controller.take_outbound();
    assert_eq!(resent.len(), 1, "call re-sent on new connection");
    let wire_id_gen2 = resent[0].id;

    // Deliver a stale-generation reply (old wire id, old generation) — must be
    // fenced: the wire index was cleared on reconnect so the id is unknown.
    controller.deliver_reply_at_generation(wire_id_gen1, "{}", gen1);
    assert_eq!(
        controller.outstanding(),
        1,
        "stale reply fenced; call still outstanding"
    );

    // The fresh-generation reply settles the call exactly once.
    controller.deliver_reply_at_generation(wire_id_gen2, "{}", gen2);
    assert_eq!(controller.outstanding(), 0, "fresh reply settled the call");
    // The future resolves after the fresh reply (DecodeReply because `{}` cannot
    // decode to RoomCreateOut, but settlement itself happened — the fence worked).
    let result = block_on(fut);
    assert!(
        result.is_err(),
        "future resolved to an error after fresh reply: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// AC-3: replay policy — ReplayableUnderOpId vs Never (§K5)
// ---------------------------------------------------------------------------

/// AC-3 (replayable): a mutating + op_id call is held on connection loss and
/// re-sent verbatim on the new connection under a different wire id (§K5
/// `ReplayableUnderOpId`). The replay hold is non-empty between the interrupt
/// and the reconnect, and zero after.
#[test]
fn replayable_mutation_is_resent_on_reconnect() {
    let (handle, controller) = ready(KernelLimits {
        max_reconnect_attempts: 4,
        backoff_base: TickDelta::from_ticks(1),
        backoff_cap: TickDelta::from_ticks(1),
        ..KernelLimits::default()
    });

    let fut = handle.call::<RoomCreate>(
        RoomCreate {
            name: "replay-test".into(),
        },
        Dedup::Key(OpId::new("op-replay-1")),
    );
    let sent = controller.take_outbound();
    assert_eq!(sent.len(), 1);
    drop(sent); // frame consumed, id not needed

    // Interrupt: replayable call is held, not settled.
    controller.interrupt();
    assert_eq!(controller.replay_held(), 1, "replayable call held");
    assert_eq!(controller.outstanding(), 1);

    // Reconnect: held call is re-sent under a fresh wire id.
    controller.advance(1);
    controller.connect();
    assert_eq!(controller.replay_held(), 0, "hold drained on reconnect");

    let resent = controller.take_outbound();
    assert_eq!(resent.len(), 1, "call re-sent on new connection");
    let wire2 = resent[0].id;

    // Settling the re-sent call resolves the original future.
    controller.deliver_reply(wire2, "{}");
    let _ = block_on(fut); // resolves (may be DecodeReply for `{}`)
    assert_eq!(
        controller.outstanding(),
        0,
        "fully settled after replay reply"
    );
}

/// AC-3 (non-replayable): a mutating call with no op_id is never auto-replayed;
/// it settles `Disconnected { Unknown }` on connection loss and is never held
/// in the replay set (§K5 `Never`).
#[test]
fn mutation_without_op_id_never_replays_and_settles_unknown_on_disconnect() {
    let (handle, controller) = ready(KernelLimits {
        max_reconnect_attempts: 0, // any loss immediately exhausts the budget
        ..KernelLimits::default()
    });

    // mutating=true, op_id=None → ReplayPolicy::Never
    let fut = handle.call::<RoomCreate>(
        RoomCreate {
            name: "no-replay".into(),
        },
        Dedup::None,
    );
    let sent = controller.take_outbound();
    assert_eq!(sent.len(), 1, "mutating call sent");

    // A single interrupt exhausts the zero-attempt budget → Failed.
    controller.interrupt();
    assert_eq!(handle.state(), State::Failed);
    assert_eq!(
        controller.replay_held(),
        0,
        "non-replayable call is never held for replay"
    );

    let err = block_on(fut).expect_err("sent non-replayable call settles on disconnect");
    assert!(
        matches!(
            err,
            CallError::Disconnected {
                execution: Execution::Unknown
            }
        ),
        "sent non-replayable: Disconnected{{Unknown}} (may have executed), got {err:?}"
    );
    drop(sent); // silence unused-variable lint
}
