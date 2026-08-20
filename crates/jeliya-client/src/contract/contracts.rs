//! The ten #175 adapter contracts. Each is a plain async fn over a [`Rig`];
//! the test matrix (`tests/contract_suite`) runs each on every applicable
//! rig and records an explicit reason for every gap.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::{FutureExt, StreamExt};
use jeliya_api::{
    ApiError, Cursor, MessageSend, RoomActivate, RoomCreate, RoomList, StreamSubscribe,
};

use crate::contract::{fail, transitions_to, ContractResult, Rig};
use crate::event::ClientEvent;
use crate::handle::Dedup;
use crate::{CallError, Execution, State};

/// Create a room and activate it (a message target must be live — the same
/// precondition the corpus fixtures stage).
async fn live_room<R: Rig>(
    rig: &mut R,
    tag: &str,
) -> Result<jeliya_api::RoomId, crate::contract::ContractFailure> {
    let name = unique(rig.name(), tag);
    let handle = rig.handle().clone();
    let created = call_settle(
        rig,
        handle.room_create(RoomCreate { name }, Dedup::None),
        "room.create",
    )
    .await?;
    call_settle(
        rig,
        handle.call::<RoomActivate>(
            RoomActivate {
                room_id: created.room_id.clone(),
            },
            Dedup::None,
        ),
        "room.activate",
    )
    .await?;
    Ok(created.room_id)
}

/// Await ANY future under the rig's settle budget: a stranded settlement
/// fails the contract with `context` instead of hanging the suite until the
/// CI job's workflow timeout — a stranded call is exactly the adapter
/// regression this suite exists to diagnose.
pub async fn settle<R: Rig, T>(
    rig: &mut R,
    call: impl Future<Output = T>,
    context: &str,
) -> Result<T, crate::contract::ContractFailure> {
    verdict_within(rig, Box::pin(call), context).await
}

/// A backend call awaited under the rig's settle budget, with wire failures
/// surfaced as the contract's labeled failure line.
pub async fn call_settle<R: Rig, O>(
    rig: &mut R,
    call: impl Future<Output = Result<O, CallError>>,
    context: &str,
) -> Result<O, crate::contract::ContractFailure> {
    match settle(rig, call, context).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => fail!("[{}] {context} failed: {e:?}", rig.name()),
        Err(e) => Err(e),
    }
}

/// Unique suffix so repeated runs never collide on durable state.
static RUN: AtomicU64 = AtomicU64::new(0);

fn unique(rig: &str, tag: &str) -> String {
    format!(
        "175-{rig}-{tag}-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    )
}

/// Poll a call future to a verdict within the rig's budget, sleeping between
/// polls. Fails the contract with `context` if it never settles.
async fn verdict_within<R: Rig, T>(
    rig: &mut R,
    mut call: std::pin::Pin<Box<dyn futures::Future<Output = T> + '_>>,
    context: &str,
) -> Result<T, crate::contract::ContractFailure> {
    let (polls, poll_ms) = (rig.config().wait_polls, rig.config().poll_ms);
    for _ in 0..polls {
        if let Some(verdict) = call.as_mut().now_or_never() {
            return Ok(verdict);
        }
        rig.sleep_ms(poll_ms).await;
    }
    Err(crate::contract::ContractFailure(format!(
        "[{}] {} never settled within {polls} polls",
        rig.name(),
        context
    )))
}

// ---------------------------------------------------------------------------
// C1 — lifecycle reaches Ready
// ---------------------------------------------------------------------------

/// `start()` takes the handle to `Ready` (dial + hello for sockets, engine
/// activation for direct) and the transition is observable on the event
/// stream, not just on `state()`.
pub async fn c1_lifecycle_reaches_ready<R: Rig>(rig: &mut R) -> ContractResult {
    let mut sub = rig.handle().subscribe();
    rig.bring_up().await?;
    let events = rig.drain_events(&mut sub, 3).await;
    if !transitions_to(&events, State::Ready) {
        fail!(
            "[{}] state() is Ready but no StateChanged→Ready crossed the event stream: {:?}",
            rig.name(),
            events
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// C2 — typed calls round-trip on the common surface
// ---------------------------------------------------------------------------

/// `room.create` then `room.list` both resolve through the compile-time
/// paired typed surface, on a real oracle where one exists.
pub async fn c2_typed_calls_round_trip<R: Rig>(rig: &mut R) -> ContractResult {
    rig.bring_up().await?;
    let name = unique(rig.name(), "room");
    let handle = rig.handle().clone();
    let created = call_settle(
        rig,
        handle.room_create(RoomCreate { name: name.clone() }, Dedup::None),
        "room.create",
    )
    .await?;
    if rig.echoes_inputs() && created.name != name {
        fail!(
            "[{}] room.create echoed name {:?}, expected {:?}",
            rig.name(),
            created.name,
            name
        );
    }
    let listed = call_settle(rig, handle.room_list(RoomList {}, Dedup::None), "room.list").await?;
    if !listed
        .rooms
        .iter()
        .any(|room| room.room_id == created.room_id)
    {
        fail!(
            "[{}] room.list omitted the room this handle just created ({:?}); {} rooms listed",
            rig.name(),
            created.room_id,
            listed.rooms.len()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// C3 — pushes fan out to every subscription and never pose as replies
// ---------------------------------------------------------------------------

/// Two open subscriptions each observe exactly one committed Event push
/// whose `event_id` equals the `message.send` reply's — the push CAUSED by
/// that send. Correlation plus the exactly-once count proves both halves of
/// the seam rule: the push reached every consumer, and the reply itself
/// never leaked onto the event fan-out (a leaked reply would appear as a
/// second Event carrying the same id).
pub async fn c3_pushes_fan_out_never_as_reply<R: Rig>(rig: &mut R) -> ContractResult {
    rig.bring_up().await?;
    let room = live_room(rig, "push").await?;

    // Subscribe the room's push stream where the adapter supports it, then
    // open two handle subscriptions.
    if rig.supports_stream_subscribe() {
        let handle = rig.handle().clone();
        call_settle(
            rig,
            handle.stream_subscribe(
                StreamSubscribe {
                    room_id: room.clone(),
                    from: Cursor::Start,
                },
                Dedup::None,
            ),
            "stream.subscribe",
        )
        .await?;
    }
    let mut sub_a = rig.handle().subscribe();
    let mut sub_b = rig.handle().subscribe();
    // Let the subscriptions arm before the triggered push.
    let _ = rig.drain_events(&mut sub_a, 2).await;
    let _ = rig.drain_events(&mut sub_b, 2).await;

    let handle = rig.handle().clone();
    let sent = call_settle(
        rig,
        handle.message_send(
            MessageSend {
                room_id: room.clone(),
                body: "c3".to_owned(),
            },
            Dedup::None,
        ),
        "message.send",
    )
    .await?;
    if sent.room_id != room {
        fail!(
            "[{}] message.send reply named room {:?}, expected {:?}",
            rig.name(),
            sent.room_id,
            room
        );
    }

    // 20 polls (≈1 s at the default 50 ms): the push rides the same
    // connection that just delivered the reply, so latency is normally
    // milliseconds — but a loaded CI runner can stretch the fan-out past a
    // 300 ms window, and a missed push is a FALSE RED (the flake shape the
    // review flagged), not a caught defect. 1 s keeps the suite honest on
    // both sides.
    for (which, events) in [
        ("a", rig.drain_events(&mut sub_a, 20).await),
        ("b", rig.drain_events(&mut sub_b, 20).await),
    ] {
        // The push CAUSED by this send is the committed Event whose
        // event_id IS the reply's — anything else (a Peer push for the
        // room, an unrelated event) does not satisfy the contract. And it
        // must appear EXACTLY once per subscription: a second copy with
        // the same event_id is the signature of the reply itself leaking
        // onto the event fan-out, which the seam forbids ("replies never
        // travel here").
        let correlated: Vec<&crate::event::RoomPush> = events
            .iter()
            .filter_map(|event| match event {
                ClientEvent::Push(push @ crate::event::RoomPush::Event { room_id, event })
                    if *room_id == room && event.event_id == sent.event_id =>
                {
                    Some(push)
                }
                _ => None,
            })
            .collect();
        if correlated.len() != 1 {
            fail!(
                "[{}] subscription {which} saw {} Event pushes carrying the reply's event_id {:?} (expected exactly 1); events: {:?}",
                rig.name(),
                correlated.len(),
                sent.event_id,
                events
            )
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// C4 — local bounds are local, not wire errors
// ---------------------------------------------------------------------------

/// Queue-full rejects the NEXT caller locally (never a wire `ApiError`) —
/// on the real kernel path (`in_flight = 0` parks the first call, which
/// occupies the single queue slot; the second is rejected at admission) and
/// as a scripted classification on the mock.
pub async fn c4_queue_bounds_are_local_not_wire<R: Rig>(rig: &mut R) -> ContractResult {
    rig.bring_up().await?;
    let handle = rig.handle().clone();

    let parked = handle.call_stream::<RoomList>(RoomList {}, Dedup::None);
    // The parked call occupies the queue; the NEXT call must be rejected
    // locally while it sits there.
    let rejected = handle.call::<RoomList>(RoomList {}, Dedup::None);
    let verdict = verdict_within(
        rig,
        Box::pin(rejected),
        "second call with the queue occupied",
    )
    .await;
    match verdict {
        Ok(Err(CallError::QueueFull { .. })) => {}
        Ok(other) => fail!(
            "[{}] second call with the queue occupied resolved as {other:?}, expected local QueueFull",
            rig.name()
        ),
        Err(e) => return Err(e),
    }
    // Drain the parked call through stop so the rig is clean. §D6 fixes the
    // taxonomy: stop settles a hanging UNSent call as exactly
    // `Cancelled { DefinitelyNot }` (seam ac4) — a `Disconnected` or any
    // other variant with the same classification is a stop-taxonomy
    // regression, not an equivalent pass.
    handle.stop().await;
    match settle(rig, parked, "stop-settled parked call").await {
        Ok(Err(CallError::Cancelled {
            execution: Execution::DefinitelyNot,
        })) => Ok(()),
        Ok(other) => fail!(
            "[{}] parked unsent call settled as {other:?} on stop, expected exactly Cancelled{{DefinitelyNot}}",
            rig.name()
        ),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// C5 — cancellation is classified by what left the seam
// ---------------------------------------------------------------------------

/// Cancelling a call that never left the seam settles
/// `Cancelled { execution: DefinitelyNot }` on every rig: the real-kernel
/// rigs park it unsent (`in_flight = 0`), the mock hangs it unsent.
pub async fn c5_cancellation_classification<R: Rig>(rig: &mut R) -> ContractResult {
    rig.bring_up().await?;
    let handle = rig.handle().clone();
    let mut parked = handle.call_stream::<RoomList>(RoomList {}, Dedup::None);
    // Admission witness (real-kernel rigs only): while the call is parked
    // unsent it still occupies the single queue slot, so a probe call is
    // refused with a LOCAL QueueFull. If the in-flight throttle ever
    // regressed and let the parked call reach the wire, its slot would be
    // freed on the reply and the probe would sail through — failing here,
    // where the caller-declared classification alone could not notice.
    if rig.witnesses_unsent_by_admission() {
        let probe = handle.call::<RoomList>(RoomList {}, Dedup::None);
        match verdict_within(rig, Box::pin(probe), "admission probe while parked").await {
            Ok(Err(CallError::QueueFull { .. })) => {}
            Ok(other) => fail!(
                "[{}] admission probe resolved as {other:?} while a call was parked unsent — the parking premise does not hold",
                rig.name()
            ),
            Err(e) => return Err(e),
        }
    }
    let Some(cancel) = parked.cancel_handle() else {
        fail!(
            "[{}] no cancel handle on a parked call — the stream resolved before cancellation",
            rig.name()
        );
    };
    if !cancel.cancel(Execution::DefinitelyNot) {
        fail!("[{}] cancel() refused on a live parked call", rig.name());
    }
    match settle(rig, parked, "cancelled unsent call").await {
        Ok(Err(CallError::Cancelled {
            execution: Execution::DefinitelyNot,
        })) => Ok(()),
        Ok(other) => fail!(
            "[{}] unsent cancel settled as {other:?}, expected Cancelled{{DefinitelyNot}}",
            rig.name()
        ),
        Err(e) => Err(e),
    }
}

/// The sent class (cancel something the daemon may have executed) is only
/// deterministic on a scripted oracle — on a real daemon the reply can land
/// between cancellation and observation, which is honest behavior, not a
/// contract breach. Mock-only by design.
pub async fn c5_sent_cancellation_classifies_unknown<R: Rig>(rig: &mut R) -> ContractResult {
    rig.bring_up().await?;
    let handle = rig.handle().clone();
    let mut sent = handle.call_stream::<RoomList>(RoomList {}, Dedup::None);
    let Some(cancel) = sent.cancel_handle() else {
        fail!(
            "[{}] no cancel handle on a hanging sent call — it resolved before cancellation",
            rig.name()
        );
    };
    if !cancel.cancel(Execution::Unknown) {
        fail!("[{}] cancel() refused on a live sent call", rig.name());
    }
    match sent.await {
        Err(CallError::Cancelled {
            execution: Execution::Unknown,
        }) => Ok(()),
        other => fail!(
            "[{}] sent cancel settled as {other:?}, expected Cancelled{{Unknown}}",
            rig.name()
        ),
    }
}

// ---------------------------------------------------------------------------
// C6 — an exact-version gate refusal is terminal, never Ready
// ---------------------------------------------------------------------------

/// A client whose declared protocol version the daemon refuses (426
/// `ProtocolUnsupported`) lands in `Failed`, never reports `Ready`, never
/// fabricates a hello, and refuses further calls. A differential control
/// (identical rig, accepted version) reaching Ready proves the oracle was
/// alive, so the `Failed` is attributable to the refusal — not to a dead
/// daemon or exhausted transient retries.
pub async fn c6_version_mismatch_is_terminal<R: Rig>(rig: &mut R) -> ContractResult {
    let mut sub = rig.handle().subscribe();
    rig.start();
    let state = rig
        .wait_state(|s| matches!(s, State::Ready | State::Failed))
        .await?;
    if state == State::Ready {
        fail!(
            "[{}] a version-refused client reached Ready — the gate refusal was not treated as terminal",
            rig.name()
        );
    }
    rig.expect_state_never(|s| s == State::Ready, 6).await?;
    let events = rig.drain_events(&mut sub, 2).await;
    if transitions_to(&events, State::Ready) {
        fail!(
            "[{}] StateChanged→Ready crossed the event stream on a refused client: {:?}",
            rig.name(),
            events
        );
    }
    let handle = rig.handle().clone();
    let call = handle.call::<RoomList>(RoomList {}, Dedup::None);
    match verdict_within(rig, Box::pin(call), "a call on a Failed handle").await {
        Ok(Err(_)) => {}
        Ok(Ok(listed)) => fail!(
            "[{}] a call on a Failed handle succeeded with {} rooms — work ran without a connection",
            rig.name(),
            listed.rooms.len()
        ),
        Err(e) => return Err(e),
    }
    // Differential control: `Failed` alone is not a gate witness — a dead
    // oracle or a run of transient dial failures also ends in `Failed`
    // (reconnect ceiling → fail_all). An identical rig whose ONLY
    // difference is the accepted protocol version must reach Ready against
    // its own freshly spawned backend: that proves the oracle was alive
    // and attributes the refusal above to the version gate, not to the
    // environment.
    let mut control_config = rig.config().clone();
    control_config.gate_version = 2; // MIN_PROTOCOL == PROTOCOL == 2 (codec gate.rs)
    control_config.data_dir = std::path::PathBuf::from(format!(
        "{}.c6-control-{}",
        rig.config().data_dir.display(),
        unique(rig.name(), "ctl")
    ));
    let mut control = R::spawn(control_config).await?;
    control.start();
    control.wait_state(|s| s == State::Ready).await?;
    control.handle().stop().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// C7 — ambiguous execution surfaces as op_id_conflict
// ---------------------------------------------------------------------------

/// The dedup-ambiguity contract, exactly as the record defines it: a
/// retrying mutation under its `op_id` is REPLAY, not a fresh execution —
/// the same body returns the ORIGINAL result (same event, no second
/// effect), and only a different body under the used key is the wire
/// `op_id_conflict`.
pub async fn c7_ambiguous_execution_is_op_id_conflict<R: Rig>(rig: &mut R) -> ContractResult {
    rig.bring_up().await?;
    let room = live_room(rig, "c7").await?;
    let key: jeliya_api::OpId = unique(rig.name(), "op").into();
    let handle = rig.handle().clone();
    let send = |body: &'static str| MessageSend {
        room_id: room.clone(),
        body: body.to_owned(),
    };

    let first = call_settle(
        rig,
        handle.call::<MessageSend>(send("c7-first"), Dedup::Key(key.clone())),
        "first mutation under a fresh dedup key",
    )
    .await?;

    // Same body, same key: replay returns the ORIGINAL result — the same
    // event id and position, never a second authored fact.
    let replay = handle.call::<MessageSend>(send("c7-first"), Dedup::Key(key.clone()));
    let verdict = verdict_within(
        rig,
        Box::pin(replay),
        "same-body replay under the dedup key",
    )
    .await;
    match verdict {
        Ok(Ok(out)) => {
            if out.event_id != first.event_id || out.pos != first.pos {
                fail!(
                    "[{}] same-body replay under the dedup key authored a SECOND fact ({:?} @ {}) instead of returning the original ({:?} @ {})",
                    rig.name(),
                    out.event_id,
                    out.pos,
                    first.event_id,
                    first.pos
                )
            }
        }
        Ok(Err(e)) => {
            fail!(
                "[{}] same-body replay under the dedup key failed: {e:?} — replay must return the original result",
                rig.name()
            )
        }
        Err(e) => return Err(e),
    }

    // Different body, same key: the caller is pretending a fresh mutation
    // is the old one — the wire answer is op_id_conflict.
    let conflict = handle.call::<MessageSend>(send("c7-conflict"), Dedup::Key(key));
    let verdict = verdict_within(
        rig,
        Box::pin(conflict),
        "different-body retry under the dedup key",
    )
    .await;
    match verdict {
        Ok(Err(e)) if matches!(e.as_wire(), Some(ApiError::OpIdConflict { .. })) => Ok(()),
        Ok(other) => fail!(
            "[{}] different-body retry under a used dedup key resolved as {other:?}, expected wire op_id_conflict",
            rig.name()
        ),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// C8 — transport loss is honest: Interrupted → Failed, unsent work settles
// ---------------------------------------------------------------------------

/// Severing the transport (no close frame) yields `Interrupted`, then —
/// after the configured reconnect ceiling — `Failed`, with the parked
/// UNSent call settled `DefinitelyNot` and new calls refused.
pub async fn c8_transport_loss_settles_honestly<R: Rig>(rig: &mut R) -> ContractResult {
    rig.bring_up().await?;
    let mut sub = rig.handle().subscribe();
    let handle = rig.handle().clone();

    let parked = handle.call_stream::<RoomList>(RoomList {}, Dedup::None);
    drop(handle);
    rig.sever_transport().await?;
    rig.wait_state(|s| matches!(s, State::Interrupted | State::Failed))
        .await?;
    rig.wait_state(|s| s == State::Failed).await?;
    let events = rig.drain_events(&mut sub, 2).await;
    let saw_interrupted = transitions_to(&events, State::Interrupted)
        || events.iter().any(|e| {
            matches!(
                e,
                ClientEvent::StateChanged {
                    coalesced_through_problem: true,
                    ..
                }
            )
        });
    if !saw_interrupted {
        fail!(
            "[{}] loss produced no Interrupted transition and no coalesced-through-problem witness: {:?}",
            rig.name(),
            events
        );
    }
    // Loss taxonomy: the kernel settles an unsent queued call on
    // Interrupted as exactly `Disconnected { DefinitelyNot }` (core.rs
    // drop-path; the mock's drop_connection mirrors it). Stop-settled
    // `Cancelled` would be the wrong verdict here — different event,
    // different variant.
    match settle(rig, parked, "loss-settled parked call").await {
        Ok(Err(CallError::Disconnected {
            execution: Execution::DefinitelyNot,
        })) => {}
        Ok(other) => fail!(
            "[{}] parked unsent call settled as {other:?} after loss, expected exactly Disconnected{{DefinitelyNot}}",
            rig.name()
        ),
        Err(e) => return Err(e),
    }
    let handle = rig.handle().clone();
    let call = handle.call::<RoomList>(RoomList {}, Dedup::None);
    match verdict_within(rig, Box::pin(call), "a call on a Failed handle after loss").await {
        Ok(Err(_)) => Ok(()),
        Ok(Ok(_)) => fail!(
            "[{}] a call on a Failed handle succeeded after transport loss — work ran without a connection",
            rig.name()
        ),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// C9 — restart from the same durable state reconnects and reads it back
// ---------------------------------------------------------------------------

/// After the backend restarts from its persistent state, the client
/// re-resolves, re-dials, reaches Ready again, re-establishes identity, and
/// still reads what it wrote before the restart. Both real backends promise
/// durable rooms — the daemon persists its identity, room index
/// (`state.json`), and genesis store (`rooms.db`) under the data dir; the
/// direct engine's store likewise — so the read-back is asserted on every
/// rig that runs this contract.
pub async fn c9_restart_preserves_durable_state<R: Rig>(rig: &mut R) -> ContractResult {
    rig.bring_up().await?;
    let name = unique(rig.name(), "durable");
    let pre_handle = rig.handle().clone();
    let created = call_settle(
        rig,
        pre_handle.room_create(RoomCreate { name: name.clone() }, Dedup::None),
        "pre-restart room.create",
    )
    .await?;
    let room = created.room_id.clone();

    rig.restart_backend().await?;
    rig.wait_state(|s| s == State::Ready).await?;
    // The direct rig REPLACES its handle across restart_backend, so the
    // post-restart reads must use the CURRENT handle — a pre-restart clone
    // would talk to the retired backend and settle Disconnected forever.
    let handle = rig.handle().clone();
    // Identity first: the subject is per-session state on the daemon, so a
    // reconnected session re-establishes it exactly as a real client's
    // bootstrap does. The rig's on_ready self-stabilizes across the
    // reconnect's final cycles (typed, bounded retries) and returns only
    // final verdicts, so no string-matching happens here.
    rig.on_ready().await?;
    // Then the durable read-back, with one stabilization window: calls
    // racing the reconnect's final cycle settle `Disconnected`, and the
    // daemon-incarnation fence (#270) provokes one controlled re-dial
    // after a restart that cancels still-unsent queued calls as
    // `Cancelled { DefinitelyNot }`. Both are "provably nothing executed"
    // verdicts; durability is the contract — not which millisecond the
    // first retry landed — so both retry. Every other verdict is final.
    let mut listed = None;
    for _ in 0..rig.config().wait_polls {
        match settle(
            rig,
            handle.room_list(RoomList {}, Dedup::None),
            "post-restart room.list",
        )
        .await
        {
            Ok(Ok(out)) => {
                listed = Some(out);
                break;
            }
            Ok(Err(
                e @ (CallError::Disconnected { .. }
                | CallError::Cancelled {
                    execution: Execution::DefinitelyNot,
                }),
            )) => {
                let _ = e;
                rig.sleep_ms(rig.config().poll_ms).await;
            }
            Ok(Err(e)) => fail!("[{}] post-restart room.list failed: {e:?}", rig.name()),
            Err(e) => return Err(e),
        }
    }
    let listed = match listed {
        Some(out) => out,
        None => fail!(
            "[{}] post-restart room.list kept settling Disconnected — the reconnect never stabilized",
            rig.name()
        ),
    };
    if !listed.rooms.iter().any(|r| r.room_id == room) {
        fail!(
            "[{}] room {:?} written before the restart is absent after it; {} rooms listed — the backend promises durable rooms and did not deliver them",
            rig.name(),
            room,
            listed.rooms.len()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// C10 — stop settles accepted work, closes streams, refuses new calls
// ---------------------------------------------------------------------------

/// `stop()` settles the parked unsent call with a `DefinitelyNot`
/// classification, broadcasts `Stopping` then `Stopped`, ends every event
/// stream, refuses new calls, and is idempotent.
pub async fn c10_stop_settles_and_refuses<R: Rig>(rig: &mut R) -> ContractResult {
    rig.bring_up().await?;
    let handle = rig.handle().clone();
    let mut sub = handle.subscribe();
    let parked = handle.call_stream::<RoomList>(RoomList {}, Dedup::None);

    handle.stop().await;
    if rig.handle().state() != State::Stopped {
        fail!(
            "[{}] state after stop() is {:?}, expected Stopped",
            rig.name(),
            rig.handle().state()
        );
    }
    let events = rig.drain_events(&mut sub, 2).await;
    if !transitions_to(&events, State::Stopped) {
        fail!(
            "[{}] no StateChanged→Stopped on the event stream: {:?}",
            rig.name(),
            events
        );
    }
    // The subscription must have ENDED (a following poll yields None), not
    // merely gone quiet.
    match sub.next().now_or_never() {
        Some(None) => {}
        other => fail!(
            "[{}] subscription after stop yielded {other:?}, expected stream end (None)",
            rig.name()
        ),
    }
    // §D6 taxonomy: stop settles a hanging UNSent call as exactly
    // `Cancelled { DefinitelyNot }` (seam ac4; kernel stop path). The exact
    // variant is asserted — a `Disconnected` with the same classification
    // would be a stop-taxonomy regression passing as green.
    match settle(rig, parked, "stop-settled parked call").await {
        Ok(Err(CallError::Cancelled {
            execution: Execution::DefinitelyNot,
        })) => {}
        Ok(other) => fail!(
            "[{}] parked unsent call settled as {other:?} on stop, expected exactly Cancelled{{DefinitelyNot}}",
            rig.name()
        ),
        Err(e) => return Err(e),
    }
    let handle = rig.handle().clone();
    let call = handle.call::<RoomList>(RoomList {}, Dedup::None);
    match verdict_within(rig, Box::pin(call), "a call after stop").await {
        Ok(Err(_)) => {
            // Idempotence: a second stop completes rather than hanging.
            rig.handle().stop().await;
            Ok(())
        }
        Ok(Ok(listed)) => fail!(
            "[{}] a call after stop succeeded with {} rooms — a stopped handle accepted work",
            rig.name(),
            listed.rooms.len()
        ),
        Err(e) => Err(e),
    }
}
