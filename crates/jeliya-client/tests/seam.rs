//! AC-mapped behaviour tests for the `jeliya-client` seam (#167 §7).
//!
//! Driven entirely by the deterministic mock (feature = "mock"): no wall
//! clock, no network, and no reliance on task-scheduling order. Every
//! acceptance criterion and security/correctness assertion in §7 has at
//! least one test here.
//!
//! The `[[test]]` stanza in `Cargo.toml` sets `required-features = ["mock"]`
//! so this binary is skipped (not a compile error) when the feature is off.
//! CI runs it with:
//!
//! ```text
//! cargo test --locked -p jeliya-client --features mock
//! ```

use futures::executor::block_on;
use futures::{FutureExt, StreamExt};
use jeliya_api::{
    ApiError, ByteTotal, GapReason, GapTo, OpId, Push, RoomId, RoomList, RoomListOut,
};
use jeliya_client::mock::{MockScript, Program};
use jeliya_client::{CallError, ClientEvent, Dedup, Execution, LocalError, RoomPush, State};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn empty_room_list() -> RoomListOut {
    RoomListOut { rooms: vec![] }
}

// ---------------------------------------------------------------------------
// AC-1 — Calls are compile-time paired with outputs
// ---------------------------------------------------------------------------

/// The Rust type checker is the test: `let _: RoomListOut` compiles only if
/// `call::<RoomList>` returns `Result<RoomListOut, _>`. A call cannot be made
/// without knowing its reply type at compile time — there is no untyped return.
#[test]
fn ac1_call_returns_compile_time_paired_output() {
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        controller.deliver_next();
        let out: RoomListOut = fut.await.expect("scripted reply_ok succeeds");
        assert!(out.rooms.is_empty());
    });
}

/// `Dedup::Key` on a non-deduplicated operation is accepted (protocol: `op_id`
/// "is accepted on every operation and ignored by those that do not deduplicate
/// … never `unrecognised_field`"). The mock resolves it identically to None.
#[test]
fn ac1_dedup_key_accepted_without_error() {
    use jeliya_api::OpId;
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::Key(OpId::new("stable-key")));
        controller.deliver_next();
        fut.await.expect("Dedup::Key is not an error");
    });
}

/// The convenience wrapper `room_list()` forwards to `call::<RoomList>` and
/// its return type is still paired — the wrapper does not erase the output.
#[test]
fn ac1_convenience_wrapper_preserves_pairing() {
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();

    block_on(async {
        let fut = handle.room_list(RoomList {}, Dedup::None);
        controller.deliver_next();
        let out: RoomListOut = fut.await.expect("convenience wrapper");
        assert!(out.rooms.is_empty());
    });
}

// ---------------------------------------------------------------------------
// AC-2 — Start, stop, state, gap, and subscription behavior are explicit
// ---------------------------------------------------------------------------

/// The initial state is `Idle` — no backend activity before `start()`.
#[test]
fn ac2_initial_state_is_idle() {
    let (handle, _) = MockScript::new().build();
    assert_eq!(handle.state(), State::Idle);
}

/// `start()` transitions `Idle → Connecting` and broadcasts `StateChanged`.
#[test]
fn ac2_start_transitions_idle_to_connecting() {
    let (handle, _) = MockScript::new().build();
    let mut sub = handle.subscribe();

    handle.start();

    assert_eq!(handle.state(), State::Connecting);
    let event = block_on(sub.next()).expect("StateChanged after start");
    assert!(
        matches!(
            event,
            ClientEvent::StateChanged {
                from: State::Idle,
                to: State::Connecting
            }
        ),
        "unexpected: {event:?}"
    );
}

/// `start()` is idempotent: calling it a second time emits no additional
/// `StateChanged` event and the state remains `Connecting`.
#[test]
fn ac2_start_is_idempotent() {
    let (handle, controller) = MockScript::new().build();
    let mut sub = handle.subscribe();

    handle.start();
    handle.start(); // idempotent no-op

    assert_eq!(handle.state(), State::Connecting);

    // Inject a sentinel after the two starts. If start() were not idempotent,
    // a second StateChanged event would appear before the sentinel.
    controller.emit(ClientEvent::ResyncRequired {
        room_id: RoomId::new("sentinel"),
        from_pos: 0,
    });

    block_on(async {
        let first = sub.next().await.expect("StateChanged");
        assert!(
            matches!(
                first,
                ClientEvent::StateChanged {
                    to: State::Connecting,
                    ..
                }
            ),
            "first event must be the one start() transition: {first:?}"
        );
        let second = sub.next().await.expect("sentinel");
        assert!(
            matches!(&second, ClientEvent::ResyncRequired { room_id: r, .. } if r.as_str() == "sentinel"),
            "expected sentinel immediately after start, got {second:?} — start() is not idempotent"
        );
    });
}

/// `set_state` on the controller drives arbitrary transitions and broadcasts
/// `StateChanged`, so a component can render every state from one vocabulary.
#[test]
fn ac2_set_state_broadcasts_transition() {
    let (handle, controller) = MockScript::new().build();
    let mut sub = handle.subscribe();

    controller.set_state(State::Ready);

    assert_eq!(handle.state(), State::Ready);
    let event = block_on(sub.next()).expect("StateChanged");
    assert!(
        matches!(
            event,
            ClientEvent::StateChanged {
                from: State::Idle,
                to: State::Ready
            }
        ),
        "{event:?}"
    );
}

/// A gap event injected by the controller arrives on the subscription:
/// the seam lifts a wire gap to a first-class event (§D5).
#[test]
fn ac2_gap_event_arrives_on_subscription() {
    let room_id = RoomId::new("room-1");
    let (handle, controller) = MockScript::new().build();
    let mut sub = handle.subscribe();

    controller.emit(ClientEvent::Gap {
        room_id: room_id.clone(),
        from_pos: 10,
        to: GapTo::Bounded { pos: 20 },
        reason: GapReason::Backpressure,
    });

    let event = block_on(sub.next()).expect("Gap event");
    assert!(
        matches!(&event, ClientEvent::Gap { room_id: r, from_pos: 10, .. } if r == &room_id),
        "{event:?}"
    );
}

/// `ResyncRequired` injected by the controller is a first-class event (§D5),
/// distinct from a `Gap` and from a `CallError`.
#[test]
fn ac2_resync_required_event_is_first_class() {
    let (handle, controller) = MockScript::new().build();
    let mut sub = handle.subscribe();

    controller.emit(ClientEvent::ResyncRequired {
        room_id: RoomId::new("r"),
        from_pos: 7,
    });

    let event = block_on(sub.next()).expect("ResyncRequired event");
    assert!(
        matches!(event, ClientEvent::ResyncRequired { from_pos: 7, .. }),
        "{event:?}"
    );
}

// ---------------------------------------------------------------------------
// AC-3 — Multiple consumers cannot silently steal each other's pushes
// ---------------------------------------------------------------------------

/// Two independent subscriptions both receive every emitted event. Neither
/// steals from the other — the opposite of a single shared mpsc receiver.
#[test]
fn ac3_two_subscriptions_both_receive_every_event() {
    let (handle, controller) = MockScript::new().build();
    let mut sub1 = handle.subscribe();
    let mut sub2 = handle.subscribe();

    controller.emit(ClientEvent::ResyncRequired {
        room_id: RoomId::new("r"),
        from_pos: 5,
    });

    let e1 = block_on(sub1.next()).expect("sub1");
    let e2 = block_on(sub2.next()).expect("sub2");
    assert_eq!(e1, e2, "both subscriptions see the same event");
}

/// Three independent subscriptions, three events: every subscription receives
/// all events in order, independently of the others.
#[test]
fn ac3_three_subscriptions_all_events_in_order() {
    let (handle, controller) = MockScript::new().build();
    let mut subs: Vec<_> = (0..3).map(|_| handle.subscribe()).collect();

    controller.set_state(State::Ready);
    controller.set_state(State::Interrupted);

    for sub in &mut subs {
        let e1 = block_on(sub.next()).expect("Ready");
        let e2 = block_on(sub.next()).expect("Interrupted");
        assert!(
            matches!(
                e1,
                ClientEvent::StateChanged {
                    to: State::Ready,
                    ..
                }
            ),
            "{e1:?}"
        );
        assert!(
            matches!(
                e2,
                ClientEvent::StateChanged {
                    to: State::Interrupted,
                    ..
                }
            ),
            "{e2:?}"
        );
    }
}

/// A reply is delivered only to the awaiting call future, never to any
/// subscription. After a call resolves, the subscription buffer has no
/// additional item related to the reply.
#[test]
fn ac3_replies_never_arrive_on_event_stream() {
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();
    let mut sub = handle.subscribe();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        controller.deliver_next();
        let _ = fut.await.expect("reply");

        // Inject a sentinel: it must be the very next item on the subscription,
        // proving no reply-related event leaked onto the event bus.
        controller.emit(ClientEvent::ResyncRequired {
            room_id: RoomId::new("sentinel"),
            from_pos: 0,
        });
        let event = sub.next().await.expect("sentinel");
        assert!(
            matches!(&event, ClientEvent::ResyncRequired { room_id: r, .. } if r.as_str() == "sentinel"),
            "reply must not appear before sentinel: {event:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// AC-4 — Stop settles all accepted work and closes event streams
// ---------------------------------------------------------------------------

/// `stop()` settles `Hang { sent: false }` as `Cancelled { DefinitelyNot }`
/// and `Hang { sent: true }` as `Cancelled { Unknown }`, preserving the
/// may-have-executed classification.
#[test]
fn ac4_stop_settles_hanging_calls_with_correct_execution() {
    let (handle, _) = MockScript::new()
        .on("room.list", Program::hang(false)) // unsent
        .on("room.list", Program::hang(true)) // sent
        .build();

    block_on(async {
        // Dispatch eagerly before stop; both are in the pending queue.
        let fut_unsent = handle.call::<RoomList>(RoomList {}, Dedup::None);
        let fut_sent = handle.call::<RoomList>(RoomList {}, Dedup::None);

        handle.stop().await;

        let err_unsent = fut_unsent.await.expect_err("Cancelled");
        let err_sent = fut_sent.await.expect_err("Cancelled");

        assert!(
            matches!(
                err_unsent,
                CallError::Cancelled {
                    execution: Execution::DefinitelyNot
                }
            ),
            "unsent: {err_unsent:?}"
        );
        assert!(
            matches!(
                err_sent,
                CallError::Cancelled {
                    execution: Execution::Unknown
                }
            ),
            "sent: {err_sent:?}"
        );
    });
}

/// A non-hanging call pending at `stop()` time gets its real scripted reply —
/// stop *settles*, not *aborts*, accepted work.
#[test]
fn ac4_stop_delivers_real_reply_to_non_hanging_pending_call() {
    let (handle, _) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        // stop() drains the pending queue: the non-Hang step resolves with
        // its real scripted reply, not a Cancelled.
        handle.stop().await;
        let out = fut.await.expect("real reply during stop, not Cancelled");
        assert!(out.rooms.is_empty());
    });
}

/// Any call begun after `stop()` is refused immediately with
/// `Cancelled { DefinitelyNot }` — it never left the seam.
#[test]
fn ac4_stop_refuses_new_calls_after_stop() {
    let (handle, _) = MockScript::new().build();

    block_on(async {
        handle.stop().await;
        let result = handle.call::<RoomList>(RoomList {}, Dedup::None).await;
        assert!(
            matches!(
                result,
                Err(CallError::Cancelled {
                    execution: Execution::DefinitelyNot
                })
            ),
            "{result:?}"
        );
    });
}

/// Every subscription yields `StateChanged { to: Stopping }` then
/// `StateChanged { to: Stopped }` then `None` — the stream ends after the
/// final transition.
#[test]
fn ac4_stop_closes_all_event_streams() {
    let (handle, _) = MockScript::new().build();
    let mut sub = handle.subscribe();

    block_on(async {
        handle.stop().await;

        let e1 = sub.next().await.expect("Stopping event");
        let e2 = sub.next().await.expect("Stopped event");
        let e3 = sub.next().await;

        assert!(
            matches!(
                e1,
                ClientEvent::StateChanged {
                    to: State::Stopping,
                    ..
                }
            ),
            "{e1:?}"
        );
        assert!(
            matches!(
                e2,
                ClientEvent::StateChanged {
                    to: State::Stopped,
                    ..
                }
            ),
            "{e2:?}"
        );
        assert!(e3.is_none(), "stream must end after Stopped: {e3:?}");
    });
}

/// Multiple subscriptions all close after `stop()`, independently.
#[test]
fn ac4_stop_closes_multiple_subscriptions() {
    let (handle, _) = MockScript::new().build();
    let mut sub1 = handle.subscribe();
    let mut sub2 = handle.subscribe();

    block_on(async {
        handle.stop().await;
        // Drain both streams to None.
        while sub1.next().await.is_some() {}
        while sub2.next().await.is_some() {}
    });
}

/// Final state is `Stopped` after `stop()` resolves.
#[test]
fn ac4_final_state_is_stopped() {
    let (handle, _) = MockScript::new().build();
    block_on(handle.stop());
    assert_eq!(handle.state(), State::Stopped);
}

// ---------------------------------------------------------------------------
// AC-5 — Mock scripts responses, errors, push-before-response, gaps,
//         cancellation, and shutdown
// ---------------------------------------------------------------------------

/// `Program::reply_ok` resolves the call with the typed success output.
#[test]
fn ac5_reply_ok_resolves_with_output() {
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        controller.deliver_next();
        let _ = fut.await.expect("reply_ok");
    });
}

/// `Program::reply_err` yields `CallError::Wire(ApiError)`. A daemon verdict
/// is never misclassified as a transport or local error.
#[test]
fn ac5_reply_err_delivers_as_wire() {
    let (handle, controller) = MockScript::new()
        .on("room.list", Program::reply_err(ApiError::NotReady))
        .build();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        controller.deliver_next();
        let err = fut.await.expect_err("scripted error");
        assert!(
            matches!(err, CallError::Wire(ApiError::NotReady)),
            "expected Wire(NotReady), got {err:?}"
        );
        // execution() for Wire is DefinitelyNot (a typed refusal commits no effect).
        assert_eq!(err.execution(), Execution::DefinitelyNot);
        // as_wire() is Some for Wire.
        assert!(err.as_wire().is_some());
    });
}

/// `Program::emit_then_reply` proves push-before-response deterministically:
/// the subscriber observes the push **before** the caller's future resolves,
/// because `broadcast` runs synchronously before the oneshot is resolved.
#[test]
fn ac5_push_before_response() {
    let push = ClientEvent::ResyncRequired {
        room_id: RoomId::new("r"),
        from_pos: 99,
    };
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::emit_then_reply(
                vec![push.clone()],
                Program::reply_ok::<RoomList>(&empty_room_list()),
            ),
        )
        .build();
    let mut sub = handle.subscribe();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        // deliver_next: broadcasts push events first, then resolves the call.
        controller.deliver_next();

        // The subscription buffer already contains the push event because
        // broadcast ran before the oneshot was resolved — poll it first.
        let event = sub.next().await.expect("push before response");
        assert_eq!(event, push, "subscriber sees push");

        // Now the reply is also ready.
        let _ = fut.await.expect("reply after push");
    });
}

/// Multiple pushes before one reply arrive in emission order on all
/// subscriptions.
#[test]
fn ac5_multiple_pushes_before_response_arrive_in_order() {
    let pushes: Vec<ClientEvent> = (1u64..=3)
        .map(|i| ClientEvent::ResyncRequired {
            room_id: RoomId::new("r"),
            from_pos: i,
        })
        .collect();

    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::emit_then_reply(
                pushes.clone(),
                Program::reply_ok::<RoomList>(&empty_room_list()),
            ),
        )
        .build();
    let mut sub = handle.subscribe();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        controller.deliver_next();

        for expected in &pushes {
            let got = sub.next().await.expect("push event");
            assert_eq!(&got, expected);
        }
        let _ = fut.await.expect("reply");
    });
}

/// A `ClientEvent::Gap` injected by the controller arrives on the subscription
/// as a first-class event — a component receives "you missed data" without
/// re-deriving it from a call error.
#[test]
fn ac5_gap_injected_by_controller_emit() {
    let (handle, controller) = MockScript::new().build();
    let mut sub = handle.subscribe();

    controller.emit(ClientEvent::Gap {
        room_id: RoomId::new("room-gap"),
        from_pos: 42,
        to: GapTo::Open,
        reason: GapReason::Retention,
    });

    let event = block_on(sub.next()).expect("Gap event");
    assert!(
        matches!(
            &event,
            ClientEvent::Gap {
                room_id: r,
                from_pos: 42,
                reason: GapReason::Retention,
                ..
            } if r.as_str() == "room-gap"
        ),
        "{event:?}"
    );
}

/// `Program::hang` stays pending until `stop()` or `drop_connection()`.
/// `deliver_next` returns `false` when only hanging calls are queued.
#[test]
fn ac5_hang_deliver_next_returns_false() {
    let (handle, controller) = MockScript::new()
        .on("room.list", Program::hang(false))
        .build();

    block_on(async {
        let _fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        assert!(
            !controller.deliver_next(),
            "deliver_next must return false for a Hang-only queue"
        );
        // Clean up.
        handle.stop().await;
    });
}

/// `stop()` settles a `Hang { sent: true }` as `Cancelled { Unknown }`.
#[test]
fn ac5_hang_settled_by_stop_preserves_sent_flag() {
    let (handle, controller) = MockScript::new()
        .on("room.list", Program::hang(true))
        .build();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        assert!(!controller.deliver_next()); // Hang stays pending.
        handle.stop().await;
        let err = fut.await.expect_err("Cancelled");
        assert!(
            matches!(
                err,
                CallError::Cancelled {
                    execution: Execution::Unknown
                }
            ),
            "{err:?}"
        );
    });
}

/// `StreamCall::cancel()` resolves the terminal to `CallError::Cancelled`
/// carrying the caller-supplied `Execution`.
#[test]
fn ac5_call_stream_cancel_resolves_with_classified_cancelled() {
    let (handle, _) = MockScript::new()
        .on("room.list", Program::hang(false))
        .build();

    block_on(async {
        let mut sc = handle.call_stream::<RoomList>(RoomList {}, Dedup::None);
        let ok = sc.cancel(Execution::DefinitelyNot);
        assert!(ok, "cancel() returned false");
        let err = sc.await.expect_err("Cancelled");
        assert!(
            matches!(
                err,
                CallError::Cancelled {
                    execution: Execution::DefinitelyNot
                }
            ),
            "{err:?}"
        );
    });
}

/// A detached `StreamCancel` handle can cancel a `StreamCall` from outside.
#[test]
fn ac5_stream_cancel_detached_handle() {
    let (handle, _) = MockScript::new()
        .on("room.list", Program::hang(true))
        .build();

    block_on(async {
        let mut sc = handle.call_stream::<RoomList>(RoomList {}, Dedup::None);
        let cancel = sc.cancel_handle().expect("cancel handle not yet taken");
        assert!(cancel.cancel(Execution::Unknown));
        let err = sc.await.expect_err("Cancelled");
        assert!(
            matches!(
                err,
                CallError::Cancelled {
                    execution: Execution::Unknown
                }
            ),
            "{err:?}"
        );
    });
}

/// Shutdown scenario (AC-5 "shutdown"): `handle.stop()` settles every pending
/// scripted call, emits `Stopping` then `Stopped`, and ends every subscription.
#[test]
fn ac5_shutdown_settles_pending_and_closes_subscriptions() {
    let (handle, _) = MockScript::new()
        .on("room.list", Program::hang(false))
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();
    let mut sub = handle.subscribe();

    block_on(async {
        let fut_hang = handle.call::<RoomList>(RoomList {}, Dedup::None);
        let fut_ok = handle.call::<RoomList>(RoomList {}, Dedup::None);

        handle.stop().await;

        // Hanging call → Cancelled{DefinitelyNot}.
        assert!(matches!(
            fut_hang.await,
            Err(CallError::Cancelled {
                execution: Execution::DefinitelyNot
            })
        ));
        // Non-hanging call → real reply.
        assert!(fut_ok.await.is_ok());

        // Subscription ends after Stopping + Stopped.
        let mut events = vec![];
        while let Some(e) = sub.next().await {
            events.push(e);
        }
        assert!(
            events.iter().any(|e| matches!(
                e,
                ClientEvent::StateChanged {
                    to: State::Stopping,
                    ..
                }
            )),
            "Stopping missing"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                ClientEvent::StateChanged {
                    to: State::Stopped,
                    ..
                }
            )),
            "Stopped missing"
        );
    });
}

// ---------------------------------------------------------------------------
// Security & correctness — `CallError::execution()` total and correct
// ---------------------------------------------------------------------------

/// `execution()` is a total function returning the correct `Execution` for
/// every variant (§D7). This is the type-level proof that callers can reason
/// about mutation safety without a fallible match.
#[test]
fn security_execution_is_total_and_correct_for_every_variant() {
    // Wire → DefinitelyNot (a typed refusal commits no effect).
    assert_eq!(
        CallError::Wire(ApiError::NotReady).execution(),
        Execution::DefinitelyNot
    );
    // QueueFull → DefinitelyNot (refused before a byte was sent).
    assert_eq!(
        CallError::QueueFull {
            resource: "q",
            limit: 1
        }
        .execution(),
        Execution::DefinitelyNot
    );
    // Timeout → Unknown (sent, then silence — it may have landed).
    assert_eq!(CallError::Timeout.execution(), Execution::Unknown);
    // Cancelled carries both DefinitelyNot and Unknown.
    assert_eq!(
        CallError::Cancelled {
            execution: Execution::DefinitelyNot
        }
        .execution(),
        Execution::DefinitelyNot
    );
    assert_eq!(
        CallError::Cancelled {
            execution: Execution::Unknown
        }
        .execution(),
        Execution::Unknown
    );
    // Disconnected carries both DefinitelyNot and Unknown.
    assert_eq!(
        CallError::Disconnected {
            execution: Execution::DefinitelyNot
        }
        .execution(),
        Execution::DefinitelyNot
    );
    assert_eq!(
        CallError::Disconnected {
            execution: Execution::Unknown
        }
        .execution(),
        Execution::Unknown
    );
    // Local::EncodeRequest → DefinitelyNot (nothing left the seam).
    assert_eq!(
        CallError::Local(LocalError::EncodeRequest).execution(),
        Execution::DefinitelyNot
    );
    // Local::DecodeReply → Definitely (the daemon *did* run it; only the
    // local decode failed).
    assert_eq!(
        CallError::Local(LocalError::DecodeReply).execution(),
        Execution::Definitely
    );
    // Local::Backend → Unknown (a bug near the send/receive boundary cannot
    // honestly claim otherwise).
    assert_eq!(
        CallError::Local(LocalError::Backend).execution(),
        Execution::Unknown
    );
}

/// `as_wire()` is `Some` for exactly `CallError::Wire` and `None` for every
/// other variant — a component can branch on "daemon said no" vs "transport
/// could not complete" without misreading a transport failure as a daemon verdict.
#[test]
fn security_as_wire_some_for_wire_none_for_all_others() {
    assert!(CallError::Wire(ApiError::NotReady).as_wire().is_some());
    assert!(CallError::QueueFull {
        resource: "q",
        limit: 0
    }
    .as_wire()
    .is_none());
    assert!(CallError::Timeout.as_wire().is_none());
    assert!(CallError::Cancelled {
        execution: Execution::Unknown
    }
    .as_wire()
    .is_none());
    assert!(CallError::Disconnected {
        execution: Execution::DefinitelyNot
    }
    .as_wire()
    .is_none());
    assert!(CallError::Local(LocalError::Backend).as_wire().is_none());
}

// ---------------------------------------------------------------------------
// Security — `drop_connection()` classifies sent/unsent correctly
// ---------------------------------------------------------------------------

/// `drop_connection()` settles sent-and-pending as `Disconnected { Unknown }`
/// and unsent as `Disconnected { DefinitelyNot }`: the "never-sent vs
/// may-have-executed" distinction the issue names.
#[test]
fn security_drop_connection_classifies_sent_vs_unsent() {
    let (handle, controller) = MockScript::new()
        .on("room.list", Program::hang(false)) // unsent
        .on("room.list", Program::hang(true)) // sent
        .build();

    block_on(async {
        let fut_unsent = handle.call::<RoomList>(RoomList {}, Dedup::None);
        let fut_sent = handle.call::<RoomList>(RoomList {}, Dedup::None);

        controller.drop_connection();

        let err_unsent = fut_unsent.await.expect_err("Disconnected");
        let err_sent = fut_sent.await.expect_err("Disconnected");

        assert!(
            matches!(
                err_unsent,
                CallError::Disconnected {
                    execution: Execution::DefinitelyNot
                }
            ),
            "unsent: {err_unsent:?}"
        );
        assert!(
            matches!(
                err_sent,
                CallError::Disconnected {
                    execution: Execution::Unknown
                }
            ),
            "sent: {err_sent:?}"
        );
    });
}

/// A non-Hang deliverable step in the pending queue is treated as sent by
/// `drop_connection()` and thus receives `Disconnected { Unknown }`.
#[test]
fn security_drop_connection_deliverable_step_treated_as_sent() {
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        controller.drop_connection();
        let err = fut.await.expect_err("Disconnected");
        assert!(
            matches!(
                err,
                CallError::Disconnected {
                    execution: Execution::Unknown
                }
            ),
            "{err:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Security — wire vs local/queue class separation
// ---------------------------------------------------------------------------

/// A scripted `QueueFull` yields `CallError::QueueFull`, never `Wire`. Wire
/// errors are daemon verdicts; queue pressure is a client-side classification.
#[test]
fn security_scripted_queue_full_is_not_wire() {
    let error = CallError::QueueFull {
        resource: "calls",
        limit: 10,
    };
    let (handle, controller) = MockScript::new()
        .on("room.list", Program::local(error))
        .build();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        controller.deliver_next();
        let err = fut.await.expect_err("QueueFull");
        assert!(matches!(err, CallError::QueueFull { .. }), "{err:?}");
        assert!(err.as_wire().is_none(), "QueueFull must not appear as Wire");
        assert_eq!(err.execution(), Execution::DefinitelyNot);
    });
}

/// A scripted `Timeout` yields `CallError::Timeout` (Unknown), not Wire.
#[test]
fn security_scripted_timeout_is_not_wire() {
    let (handle, controller) = MockScript::new()
        .on("room.list", Program::local(CallError::Timeout))
        .build();

    block_on(async {
        let fut = handle.call::<RoomList>(RoomList {}, Dedup::None);
        controller.deliver_next();
        let err = fut.await.expect_err("Timeout");
        assert!(matches!(err, CallError::Timeout), "{err:?}");
        assert!(err.as_wire().is_none());
        assert_eq!(err.execution(), Execution::Unknown);
    });
}

/// An unscripted operation yields `CallError::Local(Backend)` (a test-setup
/// bug surface) rather than hanging forever or panicking.
#[test]
fn security_unscripted_op_returns_local_backend_error() {
    let (handle, _) = MockScript::new().build(); // no programs registered

    block_on(async {
        let err = handle
            .call::<RoomList>(RoomList {}, Dedup::None)
            .await
            .expect_err("Local::Backend");
        assert!(
            matches!(err, CallError::Local(LocalError::Backend)),
            "{err:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// AC-6 — No per-component `cfg` logic in the verification example
// ---------------------------------------------------------------------------

/// `examples/shared_component.rs` must contain no `cfg(` token. Any `cfg`
/// there would mean the component branches on target, violating the AC that
/// "WASM-local and native compilation pass without scattered component `cfg`
/// logic."
#[test]
fn ac6_no_cfg_in_shared_component_example() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/shared_component.rs");
    let source =
        std::fs::read_to_string(path).expect("examples/shared_component.rs must be readable");
    assert!(
        !source.contains("cfg("),
        "shared_component.rs must have no cfg() directive"
    );
}

// ---------------------------------------------------------------------------
// Lifecycle — `ClientHandle::clone` semantics
// ---------------------------------------------------------------------------

/// Two clones of the same handle are `==` (pointer identity). A UI prop
/// receiving a fresh clone of the same handle must not re-render merely
/// because a clone arrived.
#[test]
fn handle_clone_shares_backend() {
    let (handle, _) = MockScript::new().build();
    let clone = handle.clone();
    assert_eq!(handle, clone);
}

/// Two independently-built handles have different backends and are `!=`.
#[test]
fn handle_built_separately_are_not_equal() {
    let (h1, _) = MockScript::new().build();
    let (h2, _) = MockScript::new().build();
    assert_ne!(h1, h2);
}

/// N clones of the same handle all share one backend: they are all `==`
/// to each other.
#[test]
fn handle_all_clones_are_eq() {
    let (handle, _) = MockScript::new().build();
    let clones: Vec<_> = (0..5).map(|_| handle.clone()).collect();
    for c in &clones {
        assert_eq!(&handle, c);
    }
}

// ---------------------------------------------------------------------------
// Lifecycle — Full lifecycle walk: Idle → Connecting → Ready →
//             Interrupted → Stopping → Stopped (one subscriber observes all)
// ---------------------------------------------------------------------------

/// A single subscription observes every state transition across the complete
/// lifecycle: `start()` drives Idle→Connecting, the controller drives
/// Connecting→Ready and Ready→Interrupted, and `stop()` drives
/// Interrupted→Stopping→Stopped before closing the stream.
#[test]
fn lifecycle_full_walk_broadcasts_all_transitions_in_order() {
    let (handle, controller) = MockScript::new().build();
    let mut sub = handle.subscribe();

    assert_eq!(handle.state(), State::Idle);

    handle.start();
    assert_eq!(handle.state(), State::Connecting);

    controller.set_state(State::Ready);
    assert_eq!(handle.state(), State::Ready);

    controller.set_state(State::Interrupted);
    assert_eq!(handle.state(), State::Interrupted);

    block_on(async {
        handle.stop().await;

        let mut states = vec![];
        while let Some(event) = sub.next().await {
            if let ClientEvent::StateChanged { to, .. } = event {
                states.push(to);
            }
        }

        assert_eq!(
            states,
            vec![
                State::Connecting,
                State::Ready,
                State::Interrupted,
                State::Stopping,
                State::Stopped,
            ],
            "full lifecycle transition sequence: {states:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Fan-out — Lagged overflow surfaces dropped events, never silently loses them
// (AC-5 honesty contract: a slow consumer is told it missed data)
// ---------------------------------------------------------------------------

/// Emitting one more event than the 1024-slot per-subscription buffer
/// triggers `ClientEvent::Lagged` on the next poll: the honesty contract
/// ("a slow consumer is told it missed pushes; they are never silently
/// discarded") is enforced structurally.
///
/// The DEFAULT_FANOUT_CAPACITY is 1024 (event.rs). When the 1025th event
/// is broadcast to a subscriber whose buffer is full, the event is dropped
/// and `dropped` is incremented. The next poll of that subscription yields
/// a `ClientEvent::Lagged { dropped: 1, .. }` instead of the dropped event.
#[test]
fn lagged_overflow_emits_lagged_event_not_silent_loss() {
    let (handle, controller) = MockScript::new().build();
    let mut sub = handle.subscribe();

    // Fill the buffer exactly, then push one more — the extra is dropped.
    for i in 0..=1024u64 {
        controller.emit(ClientEvent::ResyncRequired {
            room_id: RoomId::new("r"),
            from_pos: i,
        });
    }

    block_on(async {
        // Drain the 1024 buffered events. Lagged must not appear mid-drain.
        for _ in 0..1024u64 {
            let event = sub.next().await.expect("buffered event");
            assert!(
                !matches!(event, ClientEvent::Lagged { .. }),
                "Lagged must not appear before buffer fully drains"
            );
        }
        // The very next item must be Lagged{dropped:1}, never the 1025th event.
        let lagged = sub.next().await.expect("Lagged after overflow");
        assert!(
            matches!(lagged, ClientEvent::Lagged { dropped: 1, .. }),
            "expected Lagged {{ dropped: 1 }}, got {lagged:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Wire push routing — From<Push> for ClientEvent (§D5)
//
// The seam lifts wire pushes to first-class events and routes them to the
// correct arm. The `From<Push>` impl is the one place in the codebase where
// this routing lives; these tests prove it never conflates arms.
// ---------------------------------------------------------------------------

/// `Push::Gap` routes to `ClientEvent::Gap`, not `ClientEvent::Push(_)`.
/// A position discontinuity renders differently from "here is new data",
/// which is why the seam gives it its own arm (§D5).
#[test]
fn wire_push_gap_routes_to_client_event_gap_not_push() {
    let room_id = RoomId::new("room-gap");
    let push = Push::Gap {
        room_id: room_id.clone(),
        from_pos: 10,
        to: GapTo::Bounded { pos: 20 },
        reason: GapReason::Backpressure,
    };
    let event: ClientEvent = push.into();
    assert!(
        matches!(
            &event,
            ClientEvent::Gap {
                from_pos: 10,
                to: GapTo::Bounded { pos: 20 },
                reason: GapReason::Backpressure,
                ..
            }
        ),
        "Push::Gap must route to ClientEvent::Gap: {event:?}"
    );
    // Confirm it did NOT route to ClientEvent::Push.
    assert!(
        !matches!(event, ClientEvent::Push(_)),
        "Push::Gap must NOT route to ClientEvent::Push"
    );
}

/// `Push::Transfer` routes to `ClientEvent::Push(RoomPush::Transfer)`.
/// Only the principal that started the transfer receives this push; the seam
/// wraps it in `ClientEvent::Push(_)`, not a dedicated top-level arm.
#[test]
fn wire_push_transfer_routes_to_room_push_transfer() {
    let op_id = OpId::new("op-1");
    let push = Push::Transfer {
        transfer_op_id: op_id.clone(),
        transferred_bytes: 512,
        total: ByteTotal::Known { bytes: 1024 },
    };
    let event: ClientEvent = push.into();
    assert!(
        matches!(
            &event,
            ClientEvent::Push(RoomPush::Transfer {
                transferred_bytes: 512,
                total: ByteTotal::Known { bytes: 1024 },
                ..
            })
        ),
        "Push::Transfer must route to ClientEvent::Push(RoomPush::Transfer): {event:?}"
    );
    // Confirm it did NOT route to ClientEvent::Gap.
    assert!(
        !matches!(event, ClientEvent::Gap { .. }),
        "Push::Transfer must NOT route to ClientEvent::Gap"
    );
}

/// `Push::Gap` and `Push::Transfer` route to different `ClientEvent` arms:
/// the discriminant is preserved, not collapsed to a uniform push shape.
#[test]
fn wire_push_gap_and_transfer_route_to_different_arms() {
    let gap: ClientEvent = Push::Gap {
        room_id: RoomId::new("r"),
        from_pos: 0,
        to: GapTo::Open,
        reason: GapReason::Retention,
    }
    .into();
    let transfer: ClientEvent = Push::Transfer {
        transfer_op_id: OpId::new("op"),
        transferred_bytes: 0,
        total: ByteTotal::Unknown,
    }
    .into();

    assert!(
        matches!(gap, ClientEvent::Gap { .. }),
        "gap arm wrong: {gap:?}"
    );
    assert!(
        matches!(transfer, ClientEvent::Push(RoomPush::Transfer { .. })),
        "transfer arm wrong: {transfer:?}"
    );
}

// ---------------------------------------------------------------------------
// Subscription — live-only semantics (no replay of past events)
// ---------------------------------------------------------------------------

/// A subscription created **after** an event does not receive that past event.
/// The fan-out is live-only: `EventBus::subscribe` registers a new mailbox
/// with an empty buffer, so historical pushes are not replayed.
#[test]
fn subscription_created_after_event_receives_no_past_events() {
    let (handle, controller) = MockScript::new().build();

    // Emit before subscribing.
    controller.emit(ClientEvent::ResyncRequired {
        room_id: RoomId::new("past"),
        from_pos: 0,
    });

    let mut sub = handle.subscribe();

    // Emit after subscribing — this one must arrive.
    controller.emit(ClientEvent::ResyncRequired {
        room_id: RoomId::new("live"),
        from_pos: 1,
    });

    let event = block_on(sub.next()).expect("live event");
    assert!(
        matches!(
            &event,
            ClientEvent::ResyncRequired { room_id: r, from_pos: 1 }
            if r.as_str() == "live"
        ),
        "subscription must only see live events: {event:?}"
    );
}

// ---------------------------------------------------------------------------
// StreamCall — normal (non-cancelled) resolution
// ---------------------------------------------------------------------------

/// A non-hanging `call_stream` resolves to the scripted reply — the cancel
/// path is not the only exit. `StreamCall` awaited without cancellation
/// returns exactly the same output as `call`.
#[test]
fn stream_call_resolves_with_real_scripted_reply() {
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();

    block_on(async {
        let sc = handle.call_stream::<RoomList>(RoomList {}, Dedup::None);
        controller.deliver_next();
        let out: RoomListOut = sc.await.expect("real reply via StreamCall");
        assert!(out.rooms.is_empty());
    });
}

/// A `StreamCall` awaited after `cancel_handle()` is taken but the cancel
/// is never sent: the terminal resolves normally (the dropped sender does
/// not look like a cancellation).
#[test]
fn stream_call_resolves_normally_when_cancel_handle_dropped_without_cancel() {
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();

    block_on(async {
        let mut sc = handle.call_stream::<RoomList>(RoomList {}, Dedup::None);
        // Detach the cancel handle and immediately drop it — no cancel signal.
        let _cancel = sc.cancel_handle().expect("cancel handle");
        drop(_cancel);

        controller.deliver_next();
        let out: RoomListOut = sc.await.expect("normal reply after dropped cancel handle");
        assert!(out.rooms.is_empty());
    });
}

// ---------------------------------------------------------------------------
// Review-round regressions (#167 Codex round, PR #261)
//
// Each test below pins a defect found and confirmed in review: lifecycle
// events lost to overflow, mis-ordered Lagged markers, a lazy stop() refusal
// boundary, forever-pending post-close subscriptions, nested-hang scripting,
// abandoned stream dispatches, stop-owned states scripted via set_state, and
// example/docs drift.
// ---------------------------------------------------------------------------

/// Lifecycle transitions are control events: a subscriber at capacity still
/// observes `Lagged`, then `Stopping`, then the final `Stopped`, then `None` —
/// never a stream that ends without its terminal transition.
#[test]
fn overflow_preserves_terminal_lifecycle_events() {
    let (handle, controller) = MockScript::new().build();
    let mut sub = handle.subscribe();

    // Fill the mailbox exactly, then one more: 1 dropped push pending.
    for i in 0..=1024u64 {
        controller.emit(ClientEvent::ResyncRequired {
            room_id: RoomId::new("r"),
            from_pos: i,
        });
    }

    block_on(async {
        handle.stop().await;
        for _ in 0..1024u64 {
            let event = sub.next().await.expect("buffered event");
            assert!(
                !matches!(
                    event,
                    ClientEvent::Lagged { .. } | ClientEvent::StateChanged { .. }
                ),
                "pre-gap data events drain first, got {event:?}"
            );
        }
        let lagged = sub
            .next()
            .await
            .expect("Lagged flushes before control events");
        assert!(
            matches!(lagged, ClientEvent::Lagged { dropped: 1, .. }),
            "expected Lagged {{ dropped: 1 }}, got {lagged:?}"
        );
        let stopping = sub.next().await.expect("Stopping must survive overflow");
        assert!(
            matches!(
                stopping,
                ClientEvent::StateChanged {
                    to: State::Stopping,
                    ..
                }
            ),
            "expected StateChanged to Stopping, got {stopping:?}"
        );
        let stopped = sub.next().await.expect("Stopped must survive overflow");
        assert!(
            matches!(
                stopped,
                ClientEvent::StateChanged {
                    to: State::Stopped,
                    ..
                }
            ),
            "expected StateChanged to Stopped, got {stopped:?}"
        );
        assert!(sub.next().await.is_none(), "stream ends after Stopped");
    });
}

/// The `Lagged` marker occupies the position where the loss occurred: an event
/// broadcast *after* an overflow (once a slot frees) is delivered *after* the
/// `Lagged` marker, never before it.
#[test]
fn lagged_marker_precedes_events_broadcast_after_the_loss() {
    let (handle, controller) = MockScript::new().build();
    let mut sub = handle.subscribe();

    for i in 0..=1024u64 {
        controller.emit(ClientEvent::ResyncRequired {
            room_id: RoomId::new("r"),
            from_pos: i,
        });
    }

    block_on(async {
        // Free TWO slots — one for the flushed Lagged marker, one for the
        // post-gap event — then broadcast a distinguishable post-gap event.
        // (With a single free slot the marker consumes it and the new event is
        // correctly dropped again; that self-consistency is by design.)
        let first = sub.next().await.expect("first buffered event");
        assert!(matches!(
            first,
            ClientEvent::ResyncRequired { from_pos: 0, .. }
        ));
        let second = sub.next().await.expect("second buffered event");
        assert!(matches!(
            second,
            ClientEvent::ResyncRequired { from_pos: 1, .. }
        ));
        controller.emit(ClientEvent::ResyncRequired {
            room_id: RoomId::new("r"),
            from_pos: 9999,
        });

        // Drain the remaining 1022 pre-gap events.
        for _ in 0..1022u64 {
            let event = sub.next().await.expect("pre-gap event");
            assert!(
                !matches!(event, ClientEvent::Lagged { .. }),
                "Lagged must not interleave with pre-gap events"
            );
            assert!(
                !matches!(event, ClientEvent::ResyncRequired { from_pos: 9999, .. }),
                "post-gap event must not be delivered before Lagged"
            );
        }
        let lagged = sub.next().await.expect("Lagged at the loss position");
        assert!(
            matches!(lagged, ClientEvent::Lagged { dropped: 1, .. }),
            "expected Lagged {{ dropped: 1 }}, got {lagged:?}"
        );
        let post_gap = sub.next().await.expect("post-gap event after Lagged");
        assert!(
            matches!(post_gap, ClientEvent::ResyncRequired { from_pos: 9999, .. }),
            "expected the post-gap event, got {post_gap:?}"
        );
    });
}

/// The refusal boundary is established when `stop()` is *invoked*, not when
/// its future is first polled: a call issued between the two is refused as
/// `Cancelled {{ DefinitelyNot }}` even though a scripted real reply exists.
#[test]
fn stop_refusal_boundary_established_at_invocation() {
    let (handle, _controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();

    let stopping = handle.stop(); // NOT awaited yet.
    let call = handle.call::<RoomList>(RoomList {}, Dedup::None);

    block_on(async {
        stopping.await;
        let result = call.await;
        assert!(
            matches!(
                result,
                Err(CallError::Cancelled {
                    execution: Execution::DefinitelyNot
                })
            ),
            "a call issued after stop() was invoked must never execute, got {result:?}"
        );
    });
}

/// A subscription created after shutdown is born closed: it yields `None`
/// immediately instead of pending forever on a bus that will never broadcast.
#[test]
fn subscription_after_stop_is_born_closed() {
    let (handle, _controller) = MockScript::new().build();
    block_on(async {
        handle.stop().await;
        let mut sub = handle.subscribe();
        assert!(
            sub.next().await.is_none(),
            "post-close subscription must be born closed"
        );
    });
}

/// `emit_then_reply(events, hang(sent))` is a valid push-before-pending-response
/// script: `deliver_next` broadcasts the events and leaves the hang pending;
/// `stop()` then settles it as `Cancelled` with the hang's `Execution`.
#[test]
fn nested_hang_delivers_events_then_stays_pending() {
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::emit_then_reply(
                vec![ClientEvent::ResyncRequired {
                    room_id: RoomId::new("r"),
                    from_pos: 7,
                }],
                Program::hang(true),
            ),
        )
        .build();

    let mut sub = handle.subscribe();
    let mut call = handle
        .call::<RoomList>(RoomList {}, Dedup::None)
        .boxed_local();

    assert!(
        controller.deliver_next(),
        "the chain's events are deliverable"
    );
    block_on(async {
        let event = sub.next().await.expect("push delivered before the reply");
        assert!(matches!(
            event,
            ClientEvent::ResyncRequired { from_pos: 7, .. }
        ));
    });
    assert!(
        (&mut call).now_or_never().is_none(),
        "the nested hang must keep the call pending"
    );
    assert!(
        !controller.deliver_next(),
        "nothing further is deliverable while the hang pends"
    );

    block_on(async {
        handle.stop().await;
        let result = call.await;
        assert!(
            matches!(
                result,
                Err(CallError::Cancelled {
                    execution: Execution::Unknown
                })
            ),
            "hang(true) settles as Cancelled {{ Unknown }} on stop, got {result:?}"
        );
    });
}

/// Cancelling a `StreamCall` releases its backend dispatch: the next
/// `deliver_next` must not burn the abandoned call's scripted step — it
/// resolves the *next* observable call instead.
#[test]
fn cancelled_stream_call_releases_backend_slot() {
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&empty_room_list()),
        )
        .build();

    let mut stream_call = handle.call_stream::<RoomList>(RoomList {}, Dedup::None);
    assert!(stream_call.cancel(Execution::DefinitelyNot));
    let cancelled = block_on(stream_call);
    assert!(
        matches!(
            cancelled,
            Err(CallError::Cancelled {
                execution: Execution::DefinitelyNot
            })
        ),
        "{cancelled:?}"
    );

    let mut second = handle
        .call::<RoomList>(RoomList {}, Dedup::None)
        .boxed_local();
    assert!(
        controller.deliver_next(),
        "an observable call resolves — not the abandoned one"
    );
    let result = (&mut second)
        .now_or_never()
        .expect("the second call must be the one delivered");
    assert!(result.is_ok(), "{result:?}");
}

/// `set_state` refuses the stop-owned terminal states: `Stopped` without
/// settled work and closed streams is a state the contract says cannot exist.
#[test]
#[should_panic(expected = "set_state cannot script")]
fn set_state_rejects_stop_owned_states() {
    let (_handle, controller) = MockScript::new().build();
    controller.set_state(State::Stopped);
}

/// `Failed` (and other non-stop states) remain scriptable through `set_state`.
#[test]
fn set_state_failed_still_scriptable() {
    let (handle, controller) = MockScript::new().build();
    let mut sub = handle.subscribe();
    controller.set_state(State::Failed);
    block_on(async {
        let event = sub.next().await.expect("transition broadcast");
        assert!(
            matches!(
                event,
                ClientEvent::StateChanged {
                    to: State::Failed,
                    ..
                }
            ),
            "{event:?}"
        );
    });
}

/// The example's documented build commands must match its manifest gating
/// (`required-features = ["example"]`), and the component must subscribe
/// before issuing its mount call so no event is missed while the call pends.
#[test]
fn example_docs_and_subscribe_ordering_stay_consistent() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/shared_component.rs");
    let source =
        std::fs::read_to_string(path).expect("examples/shared_component.rs must be readable");
    assert!(
        source.contains("--features example"),
        "documented build commands must use the example feature"
    );
    assert!(
        !source.contains("--features mock"),
        "documented build commands must not name the mock feature"
    );
    let subscribe_at = source
        .find("handle.subscribe()")
        .expect("component must subscribe");
    let call_at = source
        .find("handle.room_list(")
        .expect("component must call");
    assert!(
        subscribe_at < call_at,
        "the component must subscribe before issuing the mount call"
    );
}
