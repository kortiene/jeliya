//! The cloneable concrete UI handle — the seam itself (#167 §D1, §D2, §D6).
//!
//! [`ClientHandle`] is a `#[derive(Clone)]` struct wrapping
//! `Arc<dyn ClientBackend>`. Cloning is an `Arc` bump; every clone shares one
//! backend, so `handle.clone()` handed to N components is one client, not N.
//! This is the exact shape Dioxus context needs — a `Clone` value stored once
//! and read by many components — and the exact shape the architecture prefers
//! over an object-unsafe generic trait.

use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;
use jeliya_api::{
    MessageSend, MessageSendOut, OpId, Operation, RoomCreate, RoomCreateOut, RoomList, RoomListOut,
    RoomTimeline, RoomTimelineOut, StreamResync, StreamResyncOut, StreamSubscribe,
    StreamSubscribeOut,
};

use crate::backend::{ClientBackend, ErasedCall, RawJson};
use crate::error::{CallError, LocalError};
use crate::event::{EventSubscription, State};

/// How a request participates in the envelope `op_id` deduplication ledger.
///
/// `op_id` is an *envelope* field, never an argument to `in` — matching the
/// protocol's "request deduplication lives in the envelope". A `Dedup::Key` on
/// an operation that does not deduplicate is accepted and ignored (the
/// protocol: `op_id` "is accepted on every operation and ignored by those that
/// do not deduplicate … never `unrecognised_field`").
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Dedup {
    /// No `op_id`. The default for non-mutating reads and for callers that
    /// accept no cross-reconnect replay.
    None,
    /// A caller-chosen, stable key. Required to make a mutation replay-safe
    /// across a reconnect; also the value `transfer.cancel` must be able to
    /// name later (through its own `in.transfer_op_id` request field, not
    /// through `Dedup`).
    Key(OpId),
}

impl Dedup {
    /// The envelope `op_id` this policy carries, if any.
    pub(crate) fn into_op_id(self) -> Option<OpId> {
        match self {
            Dedup::None => None,
            Dedup::Key(key) => Some(key),
        }
    }
}

/// The single UI-facing client contract shared Dioxus components use.
///
/// Cheap to clone; every clone is the same client. See the crate root for the
/// full contract; the load-bearing methods are [`call`](Self::call) (the one
/// compile-time-paired entry point), [`subscribe`](Self::subscribe) (an
/// independent event stream per call), [`state`](Self::state), and the
/// [`start`](Self::start) / [`stop`](Self::stop) lifecycle.
#[derive(Clone)]
pub struct ClientHandle {
    pub(crate) inner: Arc<dyn ClientBackend>,
}

impl PartialEq for ClientHandle {
    /// Two handles are equal iff they share one backend (pointer identity).
    /// This is the memoization-correct notion for a UI prop: a `handle.clone()`
    /// passed to a child is the same client, so a component holding it should
    /// not re-render merely because a fresh clone arrived. It is also what lets
    /// `ClientHandle` be a Dioxus component prop.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl std::fmt::Debug for ClientHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The erased backend is deliberately opaque; a handle prints as itself.
        f.debug_struct("ClientHandle").finish_non_exhaustive()
    }
}

impl ClientHandle {
    /// Wrap an erased backend. Adapters (and the mock) build the backend and
    /// hand it here; the erasure is entirely internal.
    pub(crate) fn from_backend(inner: Arc<dyn ClientBackend>) -> Self {
        Self { inner }
    }

    /// Invoke one approved v2 operation. The reply type is `O::Output`, bound
    /// at compile time by [`jeliya_api::Operation`] — there is no unpaired
    /// downcast and no generic "call by string" that returns an untyped value.
    /// This is *the* contract (§D2); the convenience wrappers below are thin
    /// forwarders and none may erase the output type.
    ///
    /// The returned future does the type work at the edges (§D8): it serializes
    /// `input`, hands the backend only erased bytes, and decodes the reply into
    /// `O::Output`. An encode failure is [`LocalError::EncodeRequest`]
    /// (`DefinitelyNot`); a decode failure is [`LocalError::DecodeReply`]
    /// (`Definitely` — the daemon *did* run the operation). Every other outcome
    /// is the backend's classified [`CallError`], forwarded unchanged.
    pub fn call<O: Operation>(
        &self,
        input: O,
        dedup: Dedup,
    ) -> impl Future<Output = Result<O::Output, CallError>> + '_ {
        // Issuing a call *enqueues* it: dispatch happens now, not lazily on the
        // first poll, so the request reaches the backend's outbound queue the
        // moment `call` returns. Only the reply-decode step is deferred into the
        // future. Serializing here also keeps the concrete `O` out of the
        // future's state machine.
        let dispatch = self.begin_dispatch::<O>(input, dedup);
        async move {
            let raw = dispatch?.await?;
            serde_json::from_str::<O::Output>(raw.as_str())
                .map_err(|_| CallError::Local(LocalError::DecodeReply))
        }
    }

    /// Serialize the input and hand the erased call to the backend *now*,
    /// returning the in-flight reply future (or the encode error). Shared by
    /// [`call`](Self::call) and [`call_stream`](Self::call_stream); dispatch is
    /// eager so the request is enqueued at issue time, and serialization happens
    /// here so the concrete `O` never enters the returned future's state machine.
    /// The mapping is exactly §D8: `EncodeRequest ⇒ DefinitelyNot`, every other
    /// outcome the backend's classified [`CallError`].
    fn begin_dispatch<O: Operation>(
        &self,
        input: O,
        dedup: Dedup,
    ) -> Result<BoxFuture<'static, Result<RawJson, CallError>>, CallError> {
        let text = serde_json::to_string(&input)
            .map_err(|_| CallError::Local(LocalError::EncodeRequest))?;
        Ok(self.inner.dispatch(ErasedCall {
            op: O::PATH,
            mutating: O::MUTATING,
            op_id: dedup.into_op_id(),
            input: RawJson::from_string(text),
        }))
    }

    /// The shared type work behind [`call_stream`](Self::call_stream), returned
    /// as an owned `'static` future so a [`StreamCall`] can own and move it: it
    /// dispatches eagerly (§above) and decodes the reply into `O::Output`. The
    /// mapping is §D8: `EncodeRequest ⇒ DefinitelyNot`,
    /// `DecodeReply ⇒ Definitely`, every backend error forwarded unchanged.
    pub(crate) fn dispatch_typed<O: Operation>(
        &self,
        input: O,
        dedup: Dedup,
    ) -> BoxFuture<'static, Result<O::Output, CallError>>
    where
        O::Output: 'static,
    {
        let dispatch = self.begin_dispatch::<O>(input, dedup);
        Box::pin(async move {
            let raw = dispatch?.await?;
            serde_json::from_str::<O::Output>(raw.as_str())
                .map_err(|_| CallError::Local(LocalError::DecodeReply))
        })
    }

    /// Register an independent [`EventSubscription`]. Every active subscription
    /// observes every event (a fan-out), so two components subscribing cannot
    /// silently steal each other's pushes (§D3). Replies are never delivered
    /// here — only on [`call`](Self::call).
    pub fn subscribe(&self) -> EventSubscription {
        self.inner.subscribe()
    }

    /// A snapshot of the current lifecycle [`State`]. Transitions are delivered
    /// as [`ClientEvent::StateChanged`](crate::ClientEvent::StateChanged) so a
    /// component can react without polling.
    pub fn state(&self) -> State {
        self.inner.state()
    }

    /// Begin connecting/activating. Idempotent: calling `start` on a client
    /// already `Connecting`/`Ready` is a no-op. Transitions `Idle → Connecting`
    /// (§D6).
    pub fn start(&self) {
        self.inner.start();
    }

    /// Graceful shutdown. Resolves only after every accepted call has settled
    /// (to a real reply or a delivery-classified [`CallError::Cancelled`]) and
    /// every event stream is closed (§D6). It is `async` precisely because a
    /// synchronous stop cannot truthfully say accepted work settled.
    // The seam states `stop` as an `impl Future`-returning method, uniformly
    // with `call`, so the contract reads the same for every entry point; keep
    // that shape rather than let clippy rewrite it to `async fn`.
    #[allow(clippy::manual_async_fn)]
    pub fn stop(&self) -> impl Future<Output = ()> + '_ {
        async move { self.inner.stop().await }
    }

    // -- Convenience wrappers (§D2). Each is a one-line forwarder to
    //    `call::<O>` for the highest-traffic operations; none erases the output
    //    type, so pairing is preserved. Everything else is reachable through
    //    `call::<O>` directly. --

    /// `room.create` — bring a room into existence. Forwards to
    /// [`call`](Self::call).
    pub fn room_create(
        &self,
        input: RoomCreate,
        dedup: Dedup,
    ) -> impl Future<Output = Result<RoomCreateOut, CallError>> + '_ {
        self.call::<RoomCreate>(input, dedup)
    }

    /// `room.list` — the local room list. Forwards to [`call`](Self::call).
    pub fn room_list(
        &self,
        input: RoomList,
        dedup: Dedup,
    ) -> impl Future<Output = Result<RoomListOut, CallError>> + '_ {
        self.call::<RoomList>(input, dedup)
    }

    /// `message.send` — author a message. Forwards to [`call`](Self::call).
    pub fn message_send(
        &self,
        input: MessageSend,
        dedup: Dedup,
    ) -> impl Future<Output = Result<MessageSendOut, CallError>> + '_ {
        self.call::<MessageSend>(input, dedup)
    }

    /// `room.timeline` — read committed history. Forwards to
    /// [`call`](Self::call).
    pub fn room_timeline(
        &self,
        input: RoomTimeline,
        dedup: Dedup,
    ) -> impl Future<Output = Result<RoomTimelineOut, CallError>> + '_ {
        self.call::<RoomTimeline>(input, dedup)
    }

    /// `stream.subscribe` — subscribe a connection to a room's pushes. Forwards
    /// to [`call`](Self::call).
    pub fn stream_subscribe(
        &self,
        input: StreamSubscribe,
        dedup: Dedup,
    ) -> impl Future<Output = Result<StreamSubscribeOut, CallError>> + '_ {
        self.call::<StreamSubscribe>(input, dedup)
    }

    /// `stream.resync` — the authoritative recovery read. Forwards to
    /// [`call`](Self::call). Note its `resync_required` answer arrives as
    /// [`CallError::Wire`], while a synthesized recovery instruction arrives as
    /// [`ClientEvent::ResyncRequired`](crate::ClientEvent::ResyncRequired).
    pub fn stream_resync(
        &self,
        input: StreamResync,
        dedup: Dedup,
    ) -> impl Future<Output = Result<StreamResyncOut, CallError>> + '_ {
        self.call::<StreamResync>(input, dedup)
    }
}
