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
    MessageSend, MessageSendOut, OpId, Operation, RoomArchive, RoomArchiveOut, RoomCreate,
    RoomCreateOut, RoomList, RoomListOut, RoomTimeline, RoomTimelineOut, StreamResync,
    StreamResyncOut, StreamSubscribe, StreamSubscribeOut,
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

/// Bound serde allocation amplification without materializing an intermediate
/// JSON tree. Every structural/scalar/string token is counted and nesting is
/// capped; the real serde decoder remains responsible for grammar/type checks.
fn json_shape_within(input: &str, max_tokens: u32) -> bool {
    const MAX_DEPTH: u32 = 64;
    let bytes = input.as_bytes();
    let mut index = 0_usize;
    let mut tokens = 0_u32;
    let mut depth = 0_u32;
    while index < bytes.len() {
        match bytes[index] {
            b' ' | b'\n' | b'\r' | b'\t' | b',' | b':' => index += 1,
            b'{' | b'[' => {
                tokens = tokens.saturating_add(1);
                depth = depth.saturating_add(1);
                if tokens > max_tokens || depth > MAX_DEPTH {
                    return false;
                }
                index += 1;
            }
            b'}' | b']' => {
                let Some(next) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next;
                index += 1;
            }
            b'"' => {
                tokens = tokens.saturating_add(1);
                if tokens > max_tokens {
                    return false;
                }
                index += 1;
                let mut closed = false;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index = index.saturating_add(2),
                        b'"' => {
                            index += 1;
                            closed = true;
                            break;
                        }
                        _ => index += 1,
                    }
                }
                if !closed {
                    return false;
                }
            }
            _ => {
                tokens = tokens.saturating_add(1);
                if tokens > max_tokens {
                    return false;
                }
                index += 1;
                while index < bytes.len()
                    && !matches!(
                        bytes[index],
                        b' ' | b'\n' | b'\r' | b'\t' | b',' | b':' | b'}' | b']'
                    )
                {
                    index += 1;
                }
            }
        }
    }
    depth == 0
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

    /// Register one stream call's media under its dedup key, **before**
    /// dispatching the call with the same key:
    ///
    /// ```ignore
    /// let key = OpId::new("share-7");
    /// handle.register_stream_media(key.clone(), jeliya_client::media::shared_bytes(bytes))?;
    /// let call = handle.call_stream::<FileShare>(input, Dedup::Key(key));
    /// ```
    ///
    /// A `file.share` needs a [`StreamMedia::Source`] whose `len()` equals the
    /// request's `declared_bytes`; a `file.read` needs a
    /// [`StreamMedia::Sink`](crate::media::StreamMedia) to collect the bytes.
    /// Backends whose drivers move no bytes (the mock) refuse honestly with
    /// [`LocalError::UnsupportedMedia`]. A stream dispatched without its media
    /// fails honestly at its first media effect (`source_failed`/
    /// `sink_failed`) — never a silent stall.
    pub fn register_stream_media(
        &self,
        key: jeliya_api::OpId,
        media: crate::media::StreamMedia,
    ) -> Result<(), LocalError> {
        self.inner.register_stream_media(key, media)
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

    /// Dispatch and decode one internal read with serialized-byte and JSON-token
    /// ceilings applied after the backend returns `RawJson` but before typed
    /// deserialization. Concrete transports separately bound frame accumulation.
    pub(crate) fn dispatch_typed_bounded<O: Operation>(
        &self,
        input: O,
        dedup: Dedup,
        max_reply_bytes: u64,
        max_reply_tokens: u32,
    ) -> BoxFuture<'static, Result<O::Output, CallError>>
    where
        O::Output: 'static,
    {
        let dispatch = self.begin_dispatch::<O>(input, dedup);
        Box::pin(async move {
            let raw = dispatch?.await?;
            if raw.as_str().len() as u64 > max_reply_bytes
                || !json_shape_within(raw.as_str(), max_reply_tokens)
            {
                return Err(CallError::Local(LocalError::DecodeReply));
            }
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
    /// every event stream is closed (§D6). Initiation is eager, exactly like
    /// [`call`](Self::call): the refusal boundary is established the moment
    /// `stop` is invoked — a call issued after `stop()` returns is refused as
    /// `Cancelled { DefinitelyNot }` even if the stop future has not been
    /// polled yet. Only the settle-wait is deferred into the returned future.
    pub fn stop(&self) -> impl Future<Output = ()> + '_ {
        // Calling the backend here (not inside a deferred async block) is what
        // makes initiation eager: the returned future only carries the wait.
        self.inner.stop()
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

    /// `room.archive` — open a left/removed room as a local read-only archive.
    /// A pure, network-free read that authors no event and mutates nothing, so
    /// it takes no `op_id` (callers pass [`Dedup::None`]). Forwards to
    /// [`call`](Self::call); the output type is preserved.
    pub fn room_archive(
        &self,
        input: RoomArchive,
        dedup: Dedup,
    ) -> impl Future<Output = Result<RoomArchiveOut, CallError>> + '_ {
        self.call::<RoomArchive>(input, dedup)
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

#[cfg(test)]
mod tests {
    use super::json_shape_within;

    #[test]
    fn json_shape_budget_rejects_collection_amplification() {
        assert!(json_shape_within(r#"{"items":["a","b"]}"#, 8));
        assert!(!json_shape_within(r#"{"items":["a","b"]}"#, 4));
        let deeply_nested = format!("{}0{}", "[".repeat(65), "]".repeat(65));
        assert!(!json_shape_within(&deeply_nested, 100));
    }
}
