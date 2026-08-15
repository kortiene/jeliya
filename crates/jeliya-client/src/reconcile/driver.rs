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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::poll_fn;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::StreamExt;
use jeliya_api::{RoomId, RoomMembers, RoomPeers, RoomTimeline, StreamResync};

use crate::event::{ClientEvent, State};
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
    /// A room identifier exceeds the reconciler's configured retention bound.
    #[error("room identifier exceeds the {limit}-byte reconciler limit")]
    IdentifierTooLong {
        /// Maximum accepted UTF-8 bytes.
        limit: u32,
    },
    /// The bounded control queue is full; the command was not accepted.
    #[error("reconciler control queue is full at limit {limit}")]
    ControlQueueFull {
        /// The maximum number of pending control commands.
        limit: usize,
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
    /// An adapter decoded an envelope and room id but could not decode the
    /// room-event content. This stays private to the adapter/reconciler seam.
    DecodeFailed { room_id: RoomId },
}

/// A finite upper bound for control commands retained by one reconciler.
///
/// The configured room limit is incorporated below, but is capped so a
/// pathological `u32::MAX` configuration cannot turn construction into a
/// giant allocation.
const MAX_COMMAND_QUEUE_CAPACITY: usize = 1024;

fn command_queue_capacity(max_active_rooms: u32) -> usize {
    let rooms = usize::try_from(max_active_rooms).unwrap_or(MAX_COMMAND_QUEUE_CAPACITY);
    rooms
        .saturating_mul(2)
        .saturating_add(1)
        .clamp(1, MAX_COMMAND_QUEUE_CAPACITY)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DriverStatus {
    Running,
    StopRequested,
    Stopped,
}

/// The shared state behind every [`Reconciler`] clone.
struct Shared {
    /// Bounded control-command ingress. The handle uses `try_send`, so a fast
    /// producer receives an explicit queue-full error rather than retaining a
    /// growing backlog of room identifiers.
    commands_tx: Mutex<mpsc::Sender<Command>>,
    commands_rx: Mutex<Option<mpsc::Receiver<Command>>>,
    /// A separate one-slot stop signal cannot be blocked behind ordinary
    /// control commands. Stop must remain deliverable even when the command
    /// queue is saturated.
    stop_tx: Mutex<mpsc::Sender<()>>,
    stop_rx: Mutex<Option<mpsc::Receiver<()>>>,
    views: RoomUpdateBus,
    /// Serializes each handle-side control as one transaction: status check,
    /// active-set mutation, and finite-channel admission. Without this mutex a
    /// stop or rollback can interleave between those steps and resurrect or
    /// over-admit a room.
    control_admission: Mutex<()>,
    /// The active-room set the handle maintains for capacity enforcement and
    /// activation dedup; the `run` loop's core is the runtime source of truth.
    active: Mutex<HashSet<RoomId>>,
    /// Eager control status: once stop is requested, later controls are refused
    /// instead of being queued behind a command that will never execute.
    status: Mutex<DriverStatus>,
    handle: ClientHandle,
    limits: ReconcileLimits,
    command_capacity: usize,
}

struct RunGuard {
    shared: Arc<Shared>,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        finish_driver(&self.shared);
    }
}

fn finish_driver(shared: &Shared) {
    {
        let _admission = shared
            .control_admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *shared
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = DriverStatus::Stopped;
        shared
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
    // Wake subscribers after releasing admission: an inline waker may call a
    // control method and must observe Stopped rather than deadlock.
    shared.views.close();
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
        let command_capacity = command_queue_capacity(config.limits.max_active_rooms);
        // `futures::channel::mpsc` gives every live sender one guaranteed
        // slot in addition to the requested buffer. Keep the sole sender
        // behind a mutex (rather than cloning it per call) and request one
        // fewer shared slots so `command_capacity` is the exact bound.
        let (commands_tx, commands_rx) = mpsc::channel(command_capacity.saturating_sub(1));
        let (stop_tx, stop_rx) = mpsc::channel(0);
        Self {
            shared: Arc::new(Shared {
                commands_tx: Mutex::new(commands_tx),
                commands_rx: Mutex::new(Some(commands_rx)),
                stop_tx: Mutex::new(stop_tx),
                stop_rx: Mutex::new(Some(stop_rx)),
                views: RoomUpdateBus::with_byte_capacity(config.limits.update_mailbox_bytes),
                control_admission: Mutex::new(()),
                active: Mutex::new(HashSet::new()),
                status: Mutex::new(DriverStatus::Running),
                handle,
                limits: config.limits,
                command_capacity,
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
        let _transaction = self
            .shared
            .control_admission
            .lock()
            .expect("control admission poisoned");
        self.ensure_running()?;
        if room_id.as_str().len() as u64 > u64::from(self.shared.limits.max_identifier_bytes) {
            return Err(ReconcileError::IdentifierTooLong {
                limit: self.shared.limits.max_identifier_bytes,
            });
        }
        let inserted = {
            let mut active = self.shared.active.lock().expect("active set poisoned");
            if !active.contains(&room_id)
                && active.len() >= self.shared.limits.max_active_rooms as usize
            {
                return Err(ReconcileError::TooManyRooms {
                    limit: self.shared.limits.max_active_rooms,
                });
            }
            active.insert(room_id.clone())
        };
        if let Err(error) = self.send(Command::Activate {
            room_id: room_id.clone(),
            from_pos,
        }) {
            if inserted {
                self.shared
                    .active
                    .lock()
                    .expect("active set poisoned")
                    .remove(&room_id);
            }
            return Err(error);
        }
        Ok(())
    }

    /// Close a room: stop tracking it and cancel any outstanding read.
    pub fn deactivate_room(&self, room_id: RoomId) -> Result<(), ReconcileError> {
        let _transaction = self
            .shared
            .control_admission
            .lock()
            .expect("control admission poisoned");
        self.ensure_running()?;
        if room_id.as_str().len() as u64 > u64::from(self.shared.limits.max_identifier_bytes) {
            return Err(ReconcileError::IdentifierTooLong {
                limit: self.shared.limits.max_identifier_bytes,
            });
        }
        let was_active = self
            .shared
            .active
            .lock()
            .expect("active set poisoned")
            .remove(&room_id);
        // Avoid retaining redundant deactivations for rooms that are already
        // absent. This also keeps a caller's repeated churn from consuming the
        // finite command queue.
        if !was_active {
            return Ok(());
        }
        if let Err(error) = self.send(Command::Deactivate {
            room_id: room_id.clone(),
        }) {
            // The command was not accepted; restore the optimistic reservation
            // so a later activation is not refused or forgotten.
            self.shared
                .active
                .lock()
                .expect("active set poisoned")
                .insert(room_id);
            return Err(error);
        }
        Ok(())
    }

    /// Resume (Android/adapter): re-baseline every active room through the same
    /// authoritative path as a reconnect, **without** pretending a socket
    /// reconnected (§R11).
    pub fn resume(&self) -> Result<(), ReconcileError> {
        let _transaction = self
            .shared
            .control_admission
            .lock()
            .expect("control admission poisoned");
        self.send(Command::Resume)
    }

    /// Route a room-scoped malformed push from an adapter into the same bounded
    /// gap-recovery path as every other discontinuity. Generic frames with no
    /// usable room id remain the kernel's uncorrelated K4 drop.
    pub(crate) fn decode_failed(&self, room_id: RoomId) -> Result<(), ReconcileError> {
        let _transaction = self
            .shared
            .control_admission
            .lock()
            .expect("control admission poisoned");
        self.ensure_running()?;
        if room_id.as_str().len() as u64 > u64::from(self.shared.limits.max_identifier_bytes) {
            return Err(ReconcileError::IdentifierTooLong {
                limit: self.shared.limits.max_identifier_bytes,
            });
        }
        self.send(Command::DecodeFailed { room_id })
    }

    /// Request stop through a dedicated bounded signal. The run loop then
    /// cancels every read, forgets every room, and closes the [`RoomUpdate`]
    /// fan-out. This method returns after admission, not completion: an adapter
    /// that must stop its underlying handle afterward first awaits the
    /// [`run`](Self::run) future's completion. Dropping that future is also total
    /// and closes subscribers through an RAII guard. Later calls return
    /// [`ReconcileError::Stopped`].
    pub fn stop(&self) -> Result<(), ReconcileError> {
        let _transaction = self
            .shared
            .control_admission
            .lock()
            .expect("control admission poisoned");
        {
            let mut status = self.shared.status.lock().expect("status poisoned");
            if *status != DriverStatus::Running {
                return Err(ReconcileError::Stopped);
            }
            // Reserve the terminal transition before attempting the signal so
            // concurrent controls are refused immediately.
            *status = DriverStatus::StopRequested;
        }
        let mut stop_tx = self.shared.stop_tx.lock().expect("stop tx poisoned");
        match stop_tx.try_send(()) {
            Ok(()) => Ok(()),
            // A stop signal already queued is equivalent to this request. This
            // branch is defensive (the status mutex normally prevents it).
            Err(error) if error.is_full() => Ok(()),
            Err(_) => {
                *self.shared.status.lock().expect("status poisoned") = DriverStatus::Stopped;
                self.shared
                    .active
                    .lock()
                    .expect("active set poisoned")
                    .clear();
                Err(ReconcileError::Stopped)
            }
        }
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
            // The single receiver makes a second driver invocation a no-op;
            // it must not terminate the owner loop or reject its controls.
            return;
        };
        // Declared before all in-flight/pending collections so Rust's reverse
        // local-drop order releases their futures before terminal completion is
        // published and subscribers are closed.
        let _run_guard = RunGuard {
            shared: Arc::clone(&self.shared),
        };
        let Some(mut stop) = self.shared.stop_rx.lock().expect("stop rx poisoned").take() else {
            return;
        };
        let mut core = Core::new(self.shared.limits);
        let mut events = self.shared.handle.subscribe().fuse();
        let mut reads: HashMap<(RoomId, u64), InFlightRead> = HashMap::new();
        let mut pending_reads: BTreeMap<(RoomId, u64), PendingRead> = BTreeMap::new();
        let mut starved_events = 0_usize;

        // Reflect the seam's current lifecycle so a reconciler started after the
        // client is already `Ready` bootstraps its rooms without waiting for the
        // next transition.
        let initial = self.shared.handle.state();
        let actions = core.step(Input::Lifecycle {
            to: initial,
            coalesced_through_problem: false,
        });
        self.apply_actions(actions, &mut reads, &mut pending_reads);

        loop {
            let wakeup = next_wakeup(
                &self.shared,
                &mut stop,
                &mut commands,
                &mut events,
                &mut reads,
                &mut starved_events,
            )
            .await;
            let input = match wakeup {
                Wakeup::Stop | Wakeup::CommandClosed | Wakeup::EventClosed => {
                    let actions = core.step(Input::Stop);
                    self.apply_actions(actions, &mut reads, &mut pending_reads);
                    self.shared.views.close();
                    self.mark_stopped();
                    return;
                }
                Wakeup::Command(Command::Activate { room_id, from_pos }) => {
                    Input::ActivateRoom { room_id, from_pos }
                }
                Wakeup::Command(Command::Deactivate { room_id }) => {
                    Input::DeactivateRoom { room_id }
                }
                Wakeup::Command(Command::Resume) => Input::Resume,
                Wakeup::Command(Command::DecodeFailed { room_id }) => {
                    Input::DecodeFailed { room_id }
                }
                Wakeup::Event(ClientEvent::StateChanged {
                    from,
                    to,
                    coalesced_through_problem,
                }) => Input::LifecycleObserved {
                    from,
                    to,
                    coalesced_through_problem,
                },
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
            self.apply_actions(actions, &mut reads, &mut pending_reads);
        }
    }

    /// Perform the core's actions: issue/cancel reads and broadcast updates.
    fn apply_actions(
        &self,
        actions: Vec<Action>,
        reads: &mut HashMap<(RoomId, u64), InFlightRead>,
        pending_reads: &mut BTreeMap<(RoomId, u64), PendingRead>,
    ) {
        for action in actions {
            match action {
                Action::IssueRead {
                    room_id,
                    read_id,
                    epoch,
                    request,
                } => {
                    pending_reads.insert((room_id, read_id), PendingRead { epoch, request });
                }
                Action::CancelRead { room_id, read_id } => {
                    // Dropping the future is the local cancel: the kernel handles
                    // it as a dropped call, never a fabricated remote cancel.
                    reads.remove(&(room_id.clone(), read_id));
                    pending_reads.remove(&(room_id, read_id));
                }
                Action::EmitResyncRequired {
                    room_id,
                    generation,
                    reason,
                } => self.shared.views.broadcast(RoomUpdate::Resyncing {
                    room_id,
                    resync: ResyncRequired { generation, reason },
                }),
                Action::EmitLagged { room_id, dropped } => {
                    self.shared.views.broadcast(RoomUpdate::Lagged {
                        room_id: Some(room_id),
                        dropped,
                    });
                }
                Action::EmitView(view) => self.shared.views.broadcast(RoomUpdate::Converged(view)),
                // A stale reply already completed; there is nothing to cancel.
                Action::DropStale { .. } => {}
            }
        }
        let concurrent = self.shared.limits.max_concurrent_reads.max(1) as usize;
        while reads.len() < concurrent {
            let Some((key, pending)) = pending_reads.pop_first() else {
                break;
            };
            let fut = self.issue(pending.request);
            reads.insert(
                key,
                InFlightRead {
                    epoch: pending.epoch,
                    fut,
                    settled: None,
                },
            );
        }
    }

    /// Build the `'static` read future for one typed request. All reads go
    /// through [`ClientHandle::call`], inheriting the kernel's bounds,
    /// deadlines, cancellation, and generation fencing (§4). Reads carry no
    /// `op_id`: `stream.resync` is connection-scoped and `op_id`-ignored, so it
    /// is never kernel-replayed.
    fn issue(&self, request: ReadRequest) -> BoxFuture<'static, ReadReply> {
        let handle = self.shared.handle.clone();
        let max_reply_bytes = self.shared.limits.max_read_reply_bytes;
        let max_reply_tokens = self.shared.limits.max_read_reply_tokens;
        match request {
            ReadRequest::Timeline(req) => Box::pin(async move {
                ReadReply::Timeline(
                    handle
                        .dispatch_typed_bounded::<RoomTimeline>(
                            req,
                            Dedup::None,
                            max_reply_bytes,
                            max_reply_tokens,
                        )
                        .await,
                )
            }),
            ReadRequest::Resync(req) => Box::pin(async move {
                ReadReply::Resync(
                    handle
                        .dispatch_typed_bounded::<StreamResync>(
                            req,
                            Dedup::None,
                            max_reply_bytes,
                            max_reply_tokens,
                        )
                        .await,
                )
            }),
            ReadRequest::Members(req) => Box::pin(async move {
                ReadReply::Members(
                    handle
                        .dispatch_typed_bounded::<RoomMembers>(
                            req,
                            Dedup::None,
                            max_reply_bytes,
                            max_reply_tokens,
                        )
                        .await,
                )
            }),
            ReadRequest::Peers(req) => Box::pin(async move {
                ReadReply::Peers(
                    handle
                        .dispatch_typed_bounded::<RoomPeers>(
                            req,
                            Dedup::None,
                            max_reply_bytes,
                            max_reply_tokens,
                        )
                        .await,
                )
            }),
        }
    }

    fn ensure_running(&self) -> Result<(), ReconcileError> {
        if *self.shared.status.lock().expect("status poisoned") == DriverStatus::Running {
            Ok(())
        } else {
            Err(ReconcileError::Stopped)
        }
    }

    /// Mark the run loop terminal and release the handle-side active set.
    fn mark_stopped(&self) {
        finish_driver(&self.shared);
    }

    /// Send one ordinary control command through the finite queue. A full
    /// queue is an explicit admission failure; the command is not retained.
    fn send(&self, command: Command) -> Result<(), ReconcileError> {
        // Hold the status reservation while admitting the command. Otherwise a
        // concurrent stop could signal the terminal path between an initial
        // `ensure_running` check and this send, leaving an apparently accepted
        // command stranded behind the stop signal.
        let status = self.shared.status.lock().expect("status poisoned");
        if *status != DriverStatus::Running {
            return Err(ReconcileError::Stopped);
        }
        let mut commands_tx = self
            .shared
            .commands_tx
            .lock()
            .expect("commands tx poisoned");
        match commands_tx.try_send(command) {
            Ok(()) => Ok(()),
            Err(error) if error.is_full() => Err(ReconcileError::ControlQueueFull {
                limit: self.shared.command_capacity,
            }),
            Err(_) => {
                drop(commands_tx);
                drop(status);
                *self.shared.status.lock().expect("status poisoned") = DriverStatus::Stopped;
                self.shared
                    .active
                    .lock()
                    .expect("active set poisoned")
                    .clear();
                Err(ReconcileError::Stopped)
            }
        }
    }
}

/// One outstanding read, tagged with the epoch it was issued under.
struct InFlightRead {
    epoch: u64,
    fut: BoxFuture<'static, ReadReply>,
    /// A reply already observed ready but held until every push broadcast before
    /// it has crossed the event stream.
    settled: Option<ReadReply>,
}

struct PendingRead {
    epoch: u64,
    request: ReadRequest,
}

/// What woke the `run` loop.
enum Wakeup {
    /// The dedicated terminal stop signal arrived.
    Stop,
    /// A control command arrived.
    Command(Command),
    /// The command channel closed (every handle dropped).
    CommandClosed,
    /// A seam event arrived.
    Event(crate::ClientEvent),
    /// The seam event subscription ended (the client stopped).
    EventClosed,
    /// A read settled.
    Read {
        room_id: RoomId,
        read_id: u64,
        epoch: u64,
        reply: ReadReply,
    },
}

/// Maximum consecutive seam events delivered while at least one baseline read
/// is outstanding before the driver polls reads first. Push-before-reply
/// ordering is preferred — it avoids spurious gap recoveries — but it must not
/// let continuously nonempty event traffic starve a settled read without
/// bound. The core is interleaving-robust (a reordered push is buffered,
/// deduplicated, or recovered as a gap), so bounded reordering is safe.
const READ_FAIRNESS_EVENT_BUDGET: usize = 32;

/// Mark the run loop's terminal path when a terminal lifecycle event crosses
/// the seam: no further control command can be admitted after it.
fn note_terminal_event(
    shared: &Shared,
    commands: &mut mpsc::Receiver<Command>,
    event: &ClientEvent,
) {
    if matches!(
        event,
        ClientEvent::StateChanged {
            to: State::Stopping | State::Stopped | State::Failed,
            ..
        }
    ) {
        commands.close();
        let mut status = shared.status.lock().expect("status poisoned");
        if *status == DriverStatus::Running {
            *status = DriverStatus::StopRequested;
        }
    }
}

/// Resolve when any source is ready. Stop and controls always lead; seam
/// events are normally polled before settled reads so a push broadcast before
/// its reply is not reordered across an authoritative snapshot, but after
/// [`READ_FAIRNESS_EVENT_BUDGET`] consecutive events with a read outstanding,
/// ready reads are consumed first (bounded fair drain). Terminal detection and
/// control admission share `control_admission`, so no command can report
/// acceptance after the run loop has selected its terminal path.
async fn next_wakeup(
    shared: &Shared,
    stop: &mut mpsc::Receiver<()>,
    commands: &mut mpsc::Receiver<Command>,
    events: &mut futures::stream::Fuse<crate::EventSubscription>,
    reads: &mut HashMap<(RoomId, u64), InFlightRead>,
    starved_events: &mut usize,
) -> Wakeup {
    poll_fn(|cx| {
        let reads_waiting = !reads.is_empty();
        let events_lead = !reads_waiting || *starved_events < READ_FAIRNESS_EVENT_BUDGET;
        // This short critical section makes command polling, terminal-source
        // polling, and the terminal status reservation linearizable with every
        // handle-side admission transaction.
        let admission = shared
            .control_admission
            .lock()
            .expect("control admission poisoned");
        match stop.poll_next_unpin(cx) {
            Poll::Ready(Some(())) | Poll::Ready(None) => {
                commands.close();
                let mut status = shared.status.lock().expect("status poisoned");
                if *status == DriverStatus::Running {
                    *status = DriverStatus::StopRequested;
                }
                return Poll::Ready(Wakeup::Stop);
            }
            Poll::Pending => {}
        }
        match commands.poll_next_unpin(cx) {
            Poll::Ready(Some(command)) => return Poll::Ready(Wakeup::Command(command)),
            Poll::Ready(None) => {
                let mut status = shared.status.lock().expect("status poisoned");
                if *status == DriverStatus::Running {
                    *status = DriverStatus::StopRequested;
                }
                return Poll::Ready(Wakeup::CommandClosed);
            }
            Poll::Pending => {}
        }
        if events_lead {
            match events.poll_next_unpin(cx) {
                Poll::Ready(Some(event)) => {
                    note_terminal_event(shared, commands, &event);
                    if reads_waiting {
                        *starved_events += 1;
                    }
                    return Poll::Ready(Wakeup::Event(event));
                }
                Poll::Ready(None) => {
                    commands.close();
                    let mut status = shared.status.lock().expect("status poisoned");
                    if *status == DriverStatus::Running {
                        *status = DriverStatus::StopRequested;
                    }
                    return Poll::Ready(Wakeup::EventClosed);
                }
                Poll::Pending => {}
            }
        }
        drop(admission);

        let mut settled_key = None;
        // HashMap iteration order is intentionally not part of the core's
        // deterministic action model. Poll ready reads by their stable
        // `(room_id, read_id)` key so simultaneous completions have the same
        // cross-room order on every target.
        let mut keys: Vec<_> = reads.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let Some(inflight) = reads.get_mut(&key) else {
                continue;
            };
            if inflight.settled.is_none() {
                if let Poll::Ready(reply) = inflight.fut.as_mut().poll(cx) {
                    inflight.settled = Some(reply);
                }
            }
            if inflight.settled.is_some() {
                settled_key = Some(key);
                break;
            }
        }
        if let Some(key) = settled_key {
            if events_lead {
                // A transport may broadcast a push and settle a reply between
                // the first event poll and this read poll. Re-poll all
                // higher-priority sources before consuming the ready future;
                // the held reply remains in `InFlightRead` if an earlier push
                // wins. With the fairness budget spent this re-poll is
                // deliberately skipped: the held read is consumed first.
                let admission = shared
                    .control_admission
                    .lock()
                    .expect("control admission poisoned");
                match stop.poll_next_unpin(cx) {
                    Poll::Ready(Some(())) | Poll::Ready(None) => {
                        commands.close();
                        let mut status = shared.status.lock().expect("status poisoned");
                        if *status == DriverStatus::Running {
                            *status = DriverStatus::StopRequested;
                        }
                        return Poll::Ready(Wakeup::Stop);
                    }
                    Poll::Pending => {}
                }
                match commands.poll_next_unpin(cx) {
                    Poll::Ready(Some(command)) => return Poll::Ready(Wakeup::Command(command)),
                    Poll::Ready(None) => {
                        let mut status = shared.status.lock().expect("status poisoned");
                        if *status == DriverStatus::Running {
                            *status = DriverStatus::StopRequested;
                        }
                        return Poll::Ready(Wakeup::CommandClosed);
                    }
                    Poll::Pending => {}
                }
                match events.poll_next_unpin(cx) {
                    Poll::Ready(Some(event)) => {
                        note_terminal_event(shared, commands, &event);
                        // The settled read stays held behind this push: that
                        // is starvation pressure, counted against the budget.
                        *starved_events += 1;
                        return Poll::Ready(Wakeup::Event(event));
                    }
                    Poll::Ready(None) => {
                        commands.close();
                        let mut status = shared.status.lock().expect("status poisoned");
                        if *status == DriverStatus::Running {
                            *status = DriverStatus::StopRequested;
                        }
                        return Poll::Ready(Wakeup::EventClosed);
                    }
                    Poll::Pending => {}
                }
                drop(admission);
            }
            let mut inflight = reads.remove(&key).expect("settled read exists");
            let reply = inflight.settled.take().expect("settled reply exists");
            *starved_events = 0;
            return Poll::Ready(Wakeup::Read {
                room_id: key.0,
                read_id: key.1,
                epoch: inflight.epoch,
                reply,
            });
        }
        if !events_lead {
            // The budget deferred the event poll, but no read was actually
            // ready: the event stream must still be polled for progress (and
            // to register its waker before returning Pending).
            let admission = shared
                .control_admission
                .lock()
                .expect("control admission poisoned");
            match events.poll_next_unpin(cx) {
                Poll::Ready(Some(event)) => {
                    note_terminal_event(shared, commands, &event);
                    *starved_events += 1;
                    return Poll::Ready(Wakeup::Event(event));
                }
                Poll::Ready(None) => {
                    commands.close();
                    let mut status = shared.status.lock().expect("status poisoned");
                    if *status == DriverStatus::Running {
                        *status = DriverStatus::StopRequested;
                    }
                    return Poll::Ready(Wakeup::EventClosed);
                }
                Poll::Pending => {}
            }
            drop(admission);
        }
        Poll::Pending
    })
    .await
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use jeliya_api::{GapReason, GapTo};

    use crate::error::{CallError, LocalError};

    #[test]
    fn push_broadcast_while_read_becomes_ready_wins_the_snapshot_race() {
        let (handle, controller) = crate::mock::MockScript::new().build();
        let reconciler = Reconciler::new(handle.clone(), ReconcileConfig::default());
        let mut stop = reconciler
            .shared
            .stop_rx
            .lock()
            .expect("stop rx")
            .take()
            .expect("stop receiver");
        let mut commands = reconciler
            .shared
            .commands_rx
            .lock()
            .expect("commands rx")
            .take()
            .expect("commands receiver");
        let mut events = handle.subscribe().fuse();
        let room_id = RoomId::new("race-room");
        let emitted_room = room_id.clone();
        let fut = Box::pin(poll_fn(move |_| {
            controller.emit(ClientEvent::Gap {
                room_id: emitted_room.clone(),
                from_pos: 0,
                to: GapTo::Open,
                reason: GapReason::Backpressure,
            });
            Poll::Ready(ReadReply::Peers(Err(CallError::Local(LocalError::Backend))))
        }));
        let mut reads = HashMap::from([(
            (room_id.clone(), 7),
            InFlightRead {
                epoch: 3,
                fut,
                settled: None,
            },
        )]);

        let mut starved = 0_usize;
        let first = block_on(next_wakeup(
            &reconciler.shared,
            &mut stop,
            &mut commands,
            &mut events,
            &mut reads,
            &mut starved,
        ));
        assert!(matches!(
            first,
            Wakeup::Event(ClientEvent::Gap { room_id: seen, .. }) if seen == room_id
        ));
        assert!(reads
            .get(&(room_id.clone(), 7))
            .is_some_and(|read| read.settled.is_some()));

        let second = block_on(next_wakeup(
            &reconciler.shared,
            &mut stop,
            &mut commands,
            &mut events,
            &mut reads,
            &mut starved,
        ));
        assert!(matches!(
            second,
            Wakeup::Read {
                read_id: 7,
                epoch: 3,
                ..
            }
        ));
    }

    /// Continuously nonempty event traffic must not starve a ready read: after
    /// the bounded fairness budget, the settled read is consumed even though
    /// more events are queued.
    #[test]
    fn continuous_event_traffic_cannot_starve_a_ready_read() {
        let (handle, controller) = crate::mock::MockScript::new().build();
        let reconciler = Reconciler::new(handle.clone(), ReconcileConfig::default());
        let mut stop = reconciler
            .shared
            .stop_rx
            .lock()
            .expect("stop rx")
            .take()
            .expect("stop receiver");
        let mut commands = reconciler
            .shared
            .commands_rx
            .lock()
            .expect("commands rx")
            .take()
            .expect("commands receiver");
        let mut events = handle.subscribe().fuse();
        let room_id = RoomId::new("fairness-room");
        let fut = Box::pin(poll_fn(|_| {
            Poll::Ready(ReadReply::Peers(Err(CallError::Local(LocalError::Backend))))
        }));
        let mut reads = HashMap::from([(
            (room_id.clone(), 1),
            InFlightRead {
                epoch: 1,
                fut,
                settled: None,
            },
        )]);
        // A queued backlog well past the fairness budget.
        for pos in 0..(READ_FAIRNESS_EVENT_BUDGET as u64 * 2) {
            controller.emit(ClientEvent::Gap {
                room_id: room_id.clone(),
                from_pos: pos,
                to: GapTo::Open,
                reason: GapReason::Backpressure,
            });
        }

        let mut starved = 0_usize;
        let mut delivered = 0_usize;
        loop {
            let wakeup = block_on(next_wakeup(
                &reconciler.shared,
                &mut stop,
                &mut commands,
                &mut events,
                &mut reads,
                &mut starved,
            ));
            match wakeup {
                Wakeup::Read { read_id: 1, .. } => break,
                Wakeup::Event(_) => {
                    delivered += 1;
                    assert!(
                        delivered <= READ_FAIRNESS_EVENT_BUDGET,
                        "a ready read must be consumed within the fairness \
                         budget; still starved after {delivered} events"
                    );
                }
                _ => panic!("unexpected wakeup"),
            }
        }
        assert_eq!(starved, 0, "consuming the read resets the fairness budget");
    }
}
