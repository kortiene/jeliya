//! The scripted (mock) rig: a deterministic oracle with no wall clock, no
//! network, and no kernel. The contracts' classification legs and the
//! sent-class cancellation run here; the enforcement legs that need the REAL
//! kernel bounds run on the native/direct rigs and are recorded N/A here
//! with reasons in the matrix.
//!
//! A driver task resolves scripted steps while the contract future awaits:
//! it parks on `MockController::pending_call` (woken by dispatch itself) and
//! delivers, exiting once the mock begins stopping so it cannot spin.

use std::future::Future;

use jeliya_api::{
    ApiError, Author, Event, EventId, EventKindContent, MessageSendOut, OpId, Role, RoomId,
    RoomListOut, Standing, StreamSubscribeOut, Timestamp,
};
use jeliya_client::contract::{ContractFailure, ContractResult, PendingPlan, Rig, RigConfig};
use jeliya_client::mock::{MockController, MockScript, Program};
use jeliya_client::{ClientHandle, State};

/// A cooperative yield for a clock-free rig: pends once, self-wakes, then
/// completes — the executor schedules other tasks (the driver) in between.
fn yield_once() -> impl Future<Output = ()> {
    let mut yielded = false;
    futures::future::poll_fn(move |cx| {
        if yielded {
            std::task::Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
}

pub struct MockRig {
    config: RigConfig,
    handle: ClientHandle,
    controller: MockController,
    #[allow(dead_code)] // aborted on drop: the driver task must not outlive the rig
    driver: tokio::task::JoinHandle<()>,
}

impl MockRig {
    /// Build the script the selected [`PendingPlan`] describes.
    fn script(plan: PendingPlan) -> MockScript {
        let mut script = MockScript::new();
        script = script
            .on(
                "room.create",
                Program::reply_ok::<jeliya_api::RoomCreate>(&jeliya_api::RoomCreateOut {
                    room_id: RoomId::from("room-mock".to_owned()),
                    name: "mock".into(),
                    role: Role::Authority,
                    standing: Standing::Active,
                    event_id: EventId::from("ev-origin".to_owned()),
                    pos: 0,
                    created_at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
                }),
            )
            .on(
                "room.activate",
                Program::reply_ok::<jeliya_api::RoomActivate>(&jeliya_api::RoomActivateOut {
                    room_id: RoomId::from("room-mock".to_owned()),
                    live: true,
                    reachability: jeliya_api::Reachability::Alone,
                    capabilities: vec![],
                }),
            )
            .on(
                "stream.subscribe",
                Program::reply_ok::<jeliya_api::StreamSubscribe>(&StreamSubscribeOut {
                    room_id: RoomId::from("room-mock".to_owned()),
                    from_pos: 0,
                }),
            )
            .on(
                "message.send",
                // The push-before-reply shape: the room event crosses the
                // event stream BEFORE the reply resolves the call.
                Program::emit_then_reply(
                    vec![jeliya_client::ClientEvent::Push(
                        jeliya_client::RoomPush::Event {
                            room_id: RoomId::from("room-mock".to_owned()),
                            event: Event {
                                pos: 1,
                                event_id: EventId::from("ev-1".to_owned()),
                                at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
                                author: Author::Unresolved,
                                kind: EventKindContent::Message {
                                    body: "c3".to_owned(),
                                },
                            },
                        },
                    )],
                    Program::reply_ok::<jeliya_api::MessageSend>(&MessageSendOut {
                        room_id: RoomId::from("room-mock".to_owned()),
                        event_id: EventId::from("ev-1".to_owned()),
                        pos: 1,
                        at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
                    }),
                ),
            )
            // C7's same-body replay: the ORIGINAL recorded result again.
            .on(
                "message.send",
                Program::reply_ok::<jeliya_api::MessageSend>(&MessageSendOut {
                    room_id: RoomId::from("room-mock".to_owned()),
                    event_id: EventId::from("ev-1".to_owned()),
                    pos: 1,
                    at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
                }),
            )
            // C7's different-body retry under the used key.
            .on(
                "message.send",
                Program::reply_err(ApiError::OpIdConflict {
                    op_id: OpId::from("op".to_owned()),
                }),
            );
        script = match plan {
            PendingPlan::Standard => script
                .on(
                    "room.list",
                    Program::reply_ok::<jeliya_api::RoomList>(&RoomListOut {
                        rooms: vec![jeliya_api::RoomRow {
                            room_id: RoomId::from("room-mock".to_owned()),
                            name: "mock".to_owned(),
                            standing: Standing::Active,
                            live: true,
                            role: Role::Authority,
                            member_count: 1,
                            last_event: jeliya_api::LastEvent::Absent,
                            capabilities: vec![],
                        }],
                    }),
                )
                // The retry contract's second call: the ledger's answer to a
                // reused dedup key.
                .on(
                    "room.list",
                    Program::reply_err(ApiError::OpIdConflict {
                        op_id: OpId::from("op".to_owned()),
                    }),
                ),
            PendingPlan::LocalQueueFull => script.on(
                "room.list",
                Program::local(jeliya_client::CallError::QueueFull {
                    resource: "calls",
                    limit: 1,
                }),
            ),
            PendingPlan::HangUnsent => script.on("room.list", Program::hang(false)),
            PendingPlan::HangSent => script.on("room.list", Program::hang(true)),
        };
        script
    }
}

impl Rig for MockRig {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn handle(&self) -> &ClientHandle {
        &self.handle
    }

    fn config(&self) -> &RigConfig {
        &self.config
    }

    async fn spawn(config: RigConfig) -> Result<Self, ContractFailure> {
        let (handle, controller) = Self::script(config.plan).build();
        let driver_controller = controller.clone();
        let driver = tokio::task::spawn(async move {
            loop {
                driver_controller.pending_call().await;
                driver_controller.deliver_next();
                if driver_controller.is_stopping() {
                    break;
                }
            }
        });
        Ok(Self {
            config,
            handle,
            controller,
            driver,
        })
    }

    fn sleep_ms(&self, ms: u64) -> impl Future<Output = ()> {
        // Clock-free: yields give the driver task scheduling points to
        // deliver scripted steps while this future's caller waits.
        let yields = ms.min(4) as usize;
        async move {
            for _ in 0..yields {
                yield_once().await;
            }
        }
    }

    fn witnesses_unsent_by_admission(&self) -> bool {
        // The mock has no admission machinery to probe; the unsent premise
        // is witnessed by the script (`Program::hang(false)`).
        false
    }

    fn echoes_inputs(&self) -> bool {
        // The script answers from fixed fixtures; it cannot echo an input
        // it never received.
        false
    }

    fn start(&mut self) {
        self.handle.start();
        // The scripted backend has no dial: reaching Ready is a scripted
        // transition the rig performs once start() has broadcast Connecting.
        self.controller.set_state(State::Ready);
    }
}

// Drop aborts the driver task via JoinHandle's own drop; the explicit impl
// documents that intent and keeps clippy from suggesting otherwise.
impl Drop for MockRig {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

/// The classification leg of C4 (mock-only): a local QueueFull is never a
/// wire error and never claims execution.
pub async fn c4_classification_mock(rig: &mut MockRig) -> ContractResult {
    use jeliya_client::{CallError, Execution};
    rig.start();
    rig.wait_state(|s| s == State::Ready).await?;
    // Bounded settle, matching the contract-side helpers: a stranded
    // scripted call fails the test instead of hanging the job.
    let handle = rig.handle().clone();
    let err = match jeliya_client::contract::contracts::settle(
        rig,
        handle.call::<jeliya_api::RoomList>(jeliya_api::RoomList {}, jeliya_client::Dedup::None),
        "scripted room.list",
    )
    .await
    {
        Ok(Err(e)) => e,
        Ok(Ok(_)) => {
            return Err(ContractFailure(
                "[mock] room.list resolved ok under the QueueFull plan".to_owned(),
            ))
        }
        Err(e) => return Err(e),
    };
    match &err {
        CallError::QueueFull { .. } => {}
        other => {
            return Err(ContractFailure(format!(
                "[mock] scripted QueueFull surfaced as {other:?}"
            )))
        }
    }
    if err.as_wire().is_some() {
        return Err(ContractFailure(
            "[mock] QueueFull posed as a wire error".to_owned(),
        ));
    }
    if err.execution() != Execution::DefinitelyNot {
        return Err(ContractFailure(format!(
            "[mock] QueueFull classified as {:?}, expected DefinitelyNot",
            err.execution()
        )));
    }
    Ok(())
}
