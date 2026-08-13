//! The async driver: the [`Reconciler`] handle and its `run` loop (#169 §4).
//!
//! The driver is the thin async shell around the sans-IO [`Core`]: it subscribes
//! **once** to the seam's [`crate::EventSubscription`], translates each
//! [`crate::ClientEvent`] into an [`Input`], applies the core's `Action`s by
//! issuing reads through [`crate::ClientHandle::call`] and broadcasting
//! [`RoomUpdate`]s, and feeds each settled read back as [`Input::ReadReply`]
//! tagged with the `read_id`/`epoch` it was issued under. It **never spawns** — a
//! single `run` future is polled by the adapter's event loop — and it owns no
//! clock and no RNG.

use std::collections::{HashMap, HashSet};
use std::future::poll_fn;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::StreamExt;
use jeliya_api::{RoomId, RoomMembers, RoomPeers, RoomTimeline, StreamResync};

use crate::handle::{ClientHandle, Dedup};
use crate::reconcile::core::{Action, Core, Input};
use crate::reconcile::reason::ResyncRequired;
use crate::reconcile::room::{ReadReply, ReadRequest};
use crate::reconcile::view::{RoomUpdate, RoomUpdateBus, RoomUpdateSubscription};
use crate::reconcile::{ReconcileConfig, ReconcileLimits};

/// A reconciler-control failure surfaced to the caller.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReconcileError {
    /// Activation would exceed [`ReconcileLimits::max_active_rooms`]; the room is
    /// **refused with this typed error, never silently dropped** (§R15).
    #[error("cannot track more than {limit} active rooms")]
    TooManyRooms {
        /// The active-room bound.
        limit: u32,
    },
    /// The reconciler's `run` loop has ended, so no further control is possible.
    #[error("the reconciler has stopped")]
    Stopped,
}

/// A control command from a [`Reconciler`] handle to its `run` loop.
enum Command {
    /// Track a room and (re)baseline it, anchored at `from_pos`.
    Activate {
        /// The room.
        room_id: RoomId,
        /// The subscription anchor.
        from_pos: u64,
    },
    /// Stop tracking a room.
    Deactivate {
        /// The room.
        room_id: RoomId,
    },
    /// Resume (Android/adapter): re-baseline every active room with no
    /// fabricated socket reconnect (§R11).
    Resume,
    /// Total stop.
    Stop,
}

/// The shared state behind every [`Reconciler`] clone.
struct Shared {
    commands_tx: mpsc::UnboundedSender<Command>,
    commands_rx: Mutex<Option<mpsc::UnboundedReceiver<Command>>>,
    views: RoomUpdateBus,
    /// The active-room set the handle maintains for capacity enforcement and
    /// activation dedup; the `run` loop's core is the runtime source of truth.
    active: Mutex<HashSet<RoomId>>,
    handle: ClientHandle,
    limits: ReconcileLimits,
}

/// The transport-independent room/session reconciler (#169).
///
/// Constructed over a [`ClientHandle`], it exposes a per-room [`RoomUpdate`]
/// stream ([`subscribe`](Self::subscribe)) and the
/// activate/deactivate/resume/stop controls. Drive it by awaiting
/// [`run`](Self::run) on the adapter's event loop; the four adapters
/// (#171/#172/#173) differ only in *which* lifecycle inputs occur, not in how
/// reconciliation runs.
///
/// Cheap to clone; every clone drives the same reconciler.
#[derive(Clone)]
pub struct Reconciler {
    shared: Arc<Shared>,
}

impl Reconciler {
    /// Construct a reconciler over a client handle.
    pub fn new(handle: ClientHandle, config: ReconcileConfig) -> Self {
        let (commands_tx, commands_rx) = mpsc::unbounded();
        Self {
            shared: Arc::new(Shared {
                commands_tx,
                commands_rx: Mutex::new(Some(commands_rx)),
                views: RoomUpdateBus::new(),
                active: Mutex::new(HashSet::new()),
                handle,
                limits: config.limits,
            }),
        }
    }

    /// Register an independent [`RoomUpdate`] subscription. Every subscription
    /// observes every reconciliation start and every converged view.
    pub fn subscribe(&self) -> RoomUpdateSubscription {
        self.shared.views.subscribe()
    }

    /// Open a room: track it and (re)baseline it, anchored at the
    /// `stream.subscribe` `from_pos`. Refused with
    /// [`ReconcileError::TooManyRooms`] if it would exceed
    /// [`ReconcileLimits::max_active_rooms`]. Re-activating a tracked room
    /// updates its anchor.
    pub fn activate_room(&self, room_id: RoomId, from_pos: u64) -> Result<(), ReconcileError> {
        {
            let mut active = self.shared.active.lock().expect("active set poisoned");
            if !active.contains(&room_id)
                && active.len() >= self.shared.limits.max_active_rooms as usize
            {
                return Err(ReconcileError::TooManyRooms {
                    limit: self.shared.limits.max_active_rooms,
                });
            }
            active.insert(room_id.clone());
        }
        self.send(Command::Activate { room_id, from_pos })
    }

    /// Close a room: stop tracking it and cancel any outstanding read.
    pub fn deactivate_room(&self, room_id: RoomId) -> Result<(), ReconcileError> {
        self.shared
            .active
            .lock()
            .expect("active set poisoned")
            .remove(&room_id);
        self.send(Command::Deactivate { room_id })
    }

    /// Resume (Android/adapter): re-baseline every active room through the same
    /// authoritative path as a reconnect, **without** pretending a socket
    /// reconnected (§R11).
    pub fn resume(&self) -> Result<(), ReconcileError> {
        self.send(Command::Resume)
    }

    /// Stop: cancel every outstanding read, forget every room, and close the
    /// [`RoomUpdate`] fan-out. The reconciler stops first (releasing its reads);
    /// the adapter then stops the handle (§R13).
    pub fn stop(&self) -> Result<(), ReconcileError> {
        self.send(Command::Stop)
    }

    /// Drive the reconciler until it stops (a [`stop`](Self::stop), the command
    /// channel closing, or the event subscription ending). Awaited once by the
    /// adapter's event loop; a second call returns immediately because the
    /// single event subscription has already been taken.
    pub async fn run(&self) {
        let Some(mut commands) = self
            .shared
            .commands_rx
            .lock()
            .expect("commands rx poisoned")
            .take()
        else {
            return;
        };
        let mut core = Core::new(self.shared.limits);
        let mut events = self.shared.handle.subscribe().fuse();
        let mut reads: HashMap<(RoomId, u64), InFlightRead> = HashMap::new();

        // Reflect the seam's current lifecycle so a reconciler started after the
        // client is already `Ready` bootstraps its rooms without waiting for the
        // next transition.
        let initial = self.shared.handle.state();
        let actions = core.step(Input::Lifecycle {
            to: initial,
            coalesced_through_problem: false,
        });
        self.apply_actions(actions, &mut reads);

        loop {
            let wakeup = next_wakeup(&mut commands, &mut events, &mut reads).await;
            let input = match wakeup {
                Wakeup::Command(Command::Activate { room_id, from_pos }) => {
                    Input::ActivateRoom { room_id, from_pos }
                }
                Wakeup::Command(Command::Deactivate { room_id }) => {
                    Input::DeactivateRoom { room_id }
                }
                Wakeup::Command(Command::Resume) => Input::Resume,
                Wakeup::Command(Command::Stop) | Wakeup::CommandsClosed | Wakeup::EventsClosed => {
                    let actions = core.step(Input::Stop);
                    self.apply_actions(actions, &mut reads);
                    self.shared.views.close();
                    return;
                }
                Wakeup::Event(event) => Input::Event(event),
                Wakeup::Read {
                    room_id,
                    read_id,
                    epoch,
                    reply,
                } => Input::ReadReply {
                    room_id,
                    read_id,
                    epoch,
                    reply,
                },
            };
            let actions = core.step(input);
            self.apply_actions(actions, &mut reads);
        }
    }

    /// Perform the core's actions: issue/cancel reads and broadcast updates.
    fn apply_actions(
        &self,
        actions: Vec<Action>,
        reads: &mut HashMap<(RoomId, u64), InFlightRead>,
    ) {
        for action in actions {
            match action {
                Action::IssueRead {
                    room_id,
                    read_id,
                    epoch,
                    request,
                } => {
                    let fut = self.issue(request);
                    reads.insert((room_id, read_id), InFlightRead { epoch, fut });
                }
                Action::CancelRead { room_id, read_id } => {
                    // Dropping the future is the local cancel: the kernel handles
                    // it as a dropped call, never a fabricated remote cancel.
                    reads.remove(&(room_id, read_id));
                }
                Action::EmitResyncRequired {
                    room_id,
                    generation,
                    reason,
                } => self.shared.views.broadcast(RoomUpdate::Resyncing {
                    room_id,
                    resync: ResyncRequired { generation, reason },
                }),
                Action::EmitView(view) => self.shared.views.broadcast(RoomUpdate::Converged(view)),
                // A stale reply already completed; there is nothing to cancel.
                Action::DropStale { .. } => {}
            }
        }
    }

    /// Build the `'static` read future for one typed request. All reads go
    /// through [`ClientHandle::call`], inheriting the kernel's bounds,
    /// deadlines, cancellation, and generation fencing (§4). Reads carry no
    /// `op_id`: `stream.resync` is connection-scoped and `op_id`-ignored, so it
    /// is never kernel-replayed.
    fn issue(&self, request: ReadRequest) -> BoxFuture<'static, ReadReply> {
        let handle = self.shared.handle.clone();
        match request {
            ReadRequest::Timeline(req) => Box::pin(async move {
                ReadReply::Timeline(handle.call::<RoomTimeline>(req, Dedup::None).await)
            }),
            ReadRequest::Resync(req) => Box::pin(async move {
                ReadReply::Resync(handle.call::<StreamResync>(req, Dedup::None).await)
            }),
            ReadRequest::Members(req) => Box::pin(async move {
                ReadReply::Members(handle.call::<RoomMembers>(req, Dedup::None).await)
            }),
            ReadRequest::Peers(req) => Box::pin(async move {
                ReadReply::Peers(handle.call::<RoomPeers>(req, Dedup::None).await)
            }),
        }
    }

    /// Send one control command, mapping a closed channel to
    /// [`ReconcileError::Stopped`].
    fn send(&self, command: Command) -> Result<(), ReconcileError> {
        self.shared
            .commands_tx
            .unbounded_send(command)
            .map_err(|_| ReconcileError::Stopped)
    }
}

/// One outstanding read, tagged with the epoch it was issued under.
struct InFlightRead {
    epoch: u64,
    fut: BoxFuture<'static, ReadReply>,
}

/// What woke the `run` loop.
enum Wakeup {
    /// A control command arrived.
    Command(Command),
    /// The command channel closed (every handle dropped).
    CommandsClosed,
    /// A seam event arrived.
    Event(crate::ClientEvent),
    /// The seam event subscription ended (the client stopped).
    EventsClosed,
    /// A read settled.
    Read {
        room_id: RoomId,
        read_id: u64,
        epoch: u64,
        reply: ReadReply,
    },
}

/// Resolve when any source is ready: a command, a settled read, or an event.
/// Commands are polled first (activate/stop take precedence), then reads (so a
/// settled baseline converges before more live pushes pile up), then events.
async fn next_wakeup(
    commands: &mut mpsc::UnboundedReceiver<Command>,
    events: &mut futures::stream::Fuse<crate::EventSubscription>,
    reads: &mut HashMap<(RoomId, u64), InFlightRead>,
) -> Wakeup {
    poll_fn(|cx| {
        match commands.poll_next_unpin(cx) {
            Poll::Ready(Some(command)) => return Poll::Ready(Wakeup::Command(command)),
            Poll::Ready(None) => return Poll::Ready(Wakeup::CommandsClosed),
            Poll::Pending => {}
        }
        let mut settled = None;
        for (key, inflight) in reads.iter_mut() {
            if let Poll::Ready(reply) = inflight.fut.as_mut().poll(cx) {
                settled = Some((key.clone(), inflight.epoch, reply));
                break;
            }
        }
        if let Some((key, epoch, reply)) = settled {
            reads.remove(&key);
            return Poll::Ready(Wakeup::Read {
                room_id: key.0,
                read_id: key.1,
                epoch,
                reply,
            });
        }
        match events.poll_next_unpin(cx) {
            Poll::Ready(Some(event)) => Poll::Ready(Wakeup::Event(event)),
            Poll::Ready(None) => Poll::Ready(Wakeup::EventsClosed),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}
