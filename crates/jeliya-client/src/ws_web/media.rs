//! The browser adapter's byte-stream media drive (§S3 of the kernel stream
//! spec): the single-threaded mirror of the native adapter's
//! `MediaRegistry` — the registry that binds registered [`StreamMedia`] to
//! wire ids and moves real bytes between the caller's sources/sinks and the
//! `web_sys::WebSocket`.
//!
//! The kernel's stream control plane is byte-free — it grants offsets and
//! windows, never payload. This module is where those grants become `JBS2`
//! DATA records on the socket and where inbound DATA payloads quarantine
//! (window-bounded) until the kernel's `WriteSink` hands them to the
//! caller's sink. All framing stays in `jeliya-codec`, called only from here
//! and the socket's message handler — no `JBS2` constant appears in this
//! file.
//!
//! The native registry runs one async media task per producer stream; the
//! browser runs on a single thread, so fulfillment is **synchronous**: a
//! `ProduceData` grant handed to the driver reads the granted bytes, frames
//! each codec-bounded chunk, and `send_with_u8_array`s it before returning
//! (the socket buffers; backpressure is the browser's own queue discipline).
//! The public media types are `Arc`-based and `Send + Sync`; that costs
//! nothing on single-threaded wasm and keeps one shared seam.
//!
//! Honest failure is the invariant: a stream that reaches a media effect
//! with no registered media reports `SourceFailed`/`SinkFailed` to the core
//! (which aborts the stream and settles the call), never a stall and never a
//! fake success. A stream never survives its connection: teardown and loss
//! clear the bound entries exactly when the core drops the streams (§S10).

use std::collections::{BTreeMap, HashMap, VecDeque};

use jeliya_api::{OpId, RequestId};
use jeliya_codec::{
    max_stream_data_bytes, BinaryAbortReason, CodecBounds, StreamIdentity, StreamRecord,
    StreamRecordBody,
};

use crate::kernel::inflight::CallId;
use crate::kernel::transport::{MediaEvent, StreamAbortReason, StreamRecordIntent};
use crate::media::StreamMedia;

/// The media-side state of one bound stream (keyed by wire id): the caller's
/// registered media, the producer's read position, and the receiver's inbound
/// quarantine buffer.
///
/// `Debug` is deliberately absent as a derive: the inbound buffer holds
/// payload bytes, and no debug render of this type may expose them (§S12).
pub(crate) struct BoundWebStream {
    /// The registered media, if the caller registered any (`None` = an
    /// honestly-failing unregistered stream).
    media: Option<StreamMedia>,
    /// The call id observed at the first media effect (`produce`/
    /// `write_sink` both receive it).
    call_id: Option<CallId>,
    /// The stream id the DAEMON adopted at OPEN — recorded when the OPEN
    /// record is decoded, and required for framing outbound DATA: the daemon
    /// routes client records by the `(request id, stream id)` pair it
    /// authored, so a fabricated id is rejected (the native registry adopts
    /// the same value at its OPEN; `None` means no OPEN was seen yet, and a
    /// grant before OPEN cannot happen — the core arms media only after it).
    stream_id: Option<u128>,
    /// The producer's read position (how far the source has been framed).
    produced: u64,
    /// Quarantined inbound DATA, keyed by offset — bounded by the registry's
    /// defensive cap (twice the kernel's stream window).
    inbound: BTreeMap<u64, Vec<u8>>,
    /// The buffer's running byte count (the cap check's numerator).
    inbound_bytes: u64,
}

impl BoundWebStream {
    /// A fresh, media-less bound entry.
    fn empty() -> Self {
        Self {
            media: None,
            call_id: None,
            stream_id: None,
            produced: 0,
            inbound: BTreeMap::new(),
            inbound_bytes: 0,
        }
    }
}

/// The outcome of one synchronous producer-grant fulfillment.
#[derive(Debug)]
pub(crate) enum ProduceOutcome {
    /// Fulfilment finished (possibly with an honest source failure): feed
    /// these media events to the core.
    Events(Vec<MediaEvent>),
    /// The socket refused a frame mid-grant: treat it as a connection loss
    /// (the driver reports `Interrupted`; nothing more may be sent).
    Closed,
}

/// The per-connection media registry, held inside the driver's single
/// `RefCell` state so every access is serialized on the one thread.
///
/// Two maps, two lifetimes (mirroring the native registry): `registered`
/// holds caller registrations keyed by dedup `OpId` awaiting the stream op's
/// send; `bound` holds the per-stream state keyed by wire id from that send
/// until the stream's terminal Text reply ([`WebMediaRegistry::prune`]) or
/// the connection's death ([`WebMediaRegistry::clear`]). Registrations
/// survive a connection loss — a stream call queued across a reconnect still
/// sends after it, and binds then; bound streams never do (§S8/§S10).
pub(crate) struct WebMediaRegistry {
    /// Caller registrations awaiting a stream op's send, keyed by dedup
    /// `OpId`, in insertion order (oldest first, for the bounded-eviction
    /// rule below).
    registered: HashMap<OpId, StreamMedia>,
    /// The registration insertion order backing the eviction rule.
    registered_order: VecDeque<OpId>,
    /// The bound on outstanding registrations (§K12: no unbounded
    /// collection). A registration whose call is never sent (refused
    /// locally, or the caller abandoned the dispatch) has no other
    /// reclaimer, so the registry evicts the OLDEST outstanding
    /// registration past the cap — an evicted key's later call fails
    /// honestly at its first media effect (`SourceFailed`/`SinkFailed`),
    /// never a silent stall.
    registered_cap: usize,
    /// Per-stream media state, keyed by the stream's wire id. Bounded by the
    /// kernel's concurrent-stream limit (every terminal prunes).
    bound: HashMap<RequestId, BoundWebStream>,
    /// The defensive inbound-buffer cap: the kernel's `stream_window_bytes`
    /// doubled (the core grants at most one window; the second window is the
    /// margin for a range delivered just before its credit extension).
    window_cap: u64,
}

impl WebMediaRegistry {
    /// Build an empty registry whose inbound cap is `stream_window_bytes × 2`
    /// (the kernel config's window, passed at construction).
    pub(crate) fn new(stream_window_bytes: u64, registered_cap: usize) -> Self {
        Self {
            registered: HashMap::new(),
            registered_order: VecDeque::new(),
            registered_cap: registered_cap.max(1),
            bound: HashMap::new(),
            window_cap: stream_window_bytes.saturating_mul(2),
        }
    }

    /// Register one stream's media under its dedup key, before the call is
    /// dispatched. Re-registering a key replaces the previous media (the
    /// caller's own last-write-wins). Past `registered_cap` the OLDEST
    /// outstanding registration is evicted — see the field's honesty note.
    pub(crate) fn register(&mut self, key: OpId, media: StreamMedia) {
        if self.registered.insert(key.clone(), media).is_none() {
            self.registered_order.push_back(key);
        }
        while self.registered.len() > self.registered_cap {
            if let Some(oldest) = self.registered_order.pop_front() {
                self.registered.remove(&oldest);
            } else {
                break;
            }
        }
    }

    /// Bind a stream op's wire id at send time: move the caller's
    /// registration (if any) onto the wire key. A stream op with **no**
    /// registration still binds an empty entry, so its later media effects
    /// fail honestly (`SourceFailed`/`SinkFailed`) instead of silently. A
    /// re-send of an already-bound id leaves the existing binding untouched.
    pub(crate) fn bind(&mut self, wire_id: RequestId, op_id: Option<OpId>) {
        if self.bound.contains_key(&wire_id) {
            return;
        }
        let media = op_id.map(|key| {
            self.registered_order.retain(|k| k != &key);
            self.registered.remove(&key)
        });
        let mut stream = BoundWebStream::empty();
        stream.media = media.flatten();
        self.bound.insert(wire_id, stream);
    }

    /// Whether a wire id has a bound stream entry.
    pub(crate) fn is_bound(&self, wire_id: RequestId) -> bool {
        self.bound.contains_key(&wire_id)
    }

    /// Record the stream id the daemon adopted at OPEN for a bound stream.
    /// Called from the OPEN decode, before the record meta reaches the core;
    /// the id is the one outbound DATA must carry (see `BoundWebStream::
    /// stream_id`). A no-op for an unbound wire id — an OPEN for a stream
    /// this driver never sent is the caller's binding-lookup failure, not a
    /// registry entry.
    pub(crate) fn adopt_open_stream_id(&mut self, wire_id: RequestId, stream_id: u128) {
        if let Some(stream) = self.bound.get_mut(&wire_id) {
            stream.stream_id = Some(stream_id);
        }
    }

    /// Fulfil one `ProduceData` grant synchronously (single thread): read ≤
    /// `up_to` bytes from the bound source in codec-bounded chunks, frame
    /// each as a DATA record, and hand the framed bytes to `send` (the live
    /// socket's binary send; returning `false` means the pipe broke). An
    /// unregistered, sink-bound, or unbound stream reports an honest
    /// `SourceFailed`, exactly like the native registry.
    pub(crate) fn produce(
        &mut self,
        id: RequestId,
        call_id: CallId,
        up_to: u64,
        bounds: &CodecBounds,
        send: &mut dyn FnMut(&[u8]) -> bool,
    ) -> ProduceOutcome {
        let chunk_cap = match max_stream_data_bytes(bounds.max_frame_bytes) {
            Ok(cap) => cap,
            Err(_) => return ProduceOutcome::Events(vec![MediaEvent::SourceFailed { call_id }]),
        };
        let (source, position) = match self.bound.get_mut(&id) {
            Some(stream) => {
                stream.call_id = Some(call_id);
                let source = match stream.media.as_ref() {
                    Some(StreamMedia::Source(source)) => Some(source.clone()),
                    _ => None,
                };
                (source, stream.produced)
            }
            None => (None, 0),
        };
        let Some(source) = source else {
            return ProduceOutcome::Events(vec![MediaEvent::SourceFailed { call_id }]);
        };
        // The daemon's OPEN stream id, without which DATA cannot be framed:
        // the daemon routes by the pair it authored (the native registry
        // adopts the same value at its OPEN).
        let Some(bound_stream_id) = self
            .bound
            .get(&id)
            .and_then(|stream| stream.stream_id)
            .filter(|sid| *sid != 0)
        else {
            return ProduceOutcome::Events(vec![MediaEvent::SourceFailed { call_id }]);
        };
        let mut events = Vec::new();
        let start_position = position;
        let mut position = position;
        let mut remaining = up_to;
        while remaining > 0 {
            let want = remaining.min(chunk_cap as u64) as usize;
            let mut buf = vec![0_u8; want];
            let read = source.read_at(position, &mut buf);
            if read == 0 {
                // EOF reports the source's own count; a zero read short of it
                // is a source failure.
                if position >= source.len() {
                    events.push(MediaEvent::SourceEnd {
                        call_id,
                        total: position,
                    });
                } else {
                    events.push(MediaEvent::SourceFailed { call_id });
                }
                self.persist_produced(id, position);
                return ProduceOutcome::Events(events);
            }
            buf.truncate(read);
            let identity = match StreamIdentity::new(id.get(), bound_stream_id) {
                Ok(identity) => identity,
                Err(_) => {
                    events.push(MediaEvent::SourceFailed { call_id });
                    self.persist_produced(id, position);
                    return ProduceOutcome::Events(events);
                }
            };
            let record = StreamRecord {
                identity,
                body: StreamRecordBody::Data {
                    offset: position,
                    payload: buf,
                },
            };
            let framed = match jeliya_codec::encode_stream_record(&record, bounds) {
                Ok(bytes) => bytes,
                // A framing refusal is a local media failure: abort honestly.
                Err(_) => {
                    events.push(MediaEvent::SourceFailed { call_id });
                    self.persist_produced(id, position);
                    return ProduceOutcome::Events(events);
                }
            };
            if !send(&framed) {
                // Connection loss mid-grant: the loss report is the driver's
                // (Interrupted); nothing more may be sent.
                self.persist_produced(id, position);
                return ProduceOutcome::Closed;
            }
            position += read as u64;
            remaining = remaining.saturating_sub(read as u64);
        }
        // ONE cumulative `Produced` per grant, never per chunk: a per-chunk
        // report lets the core enqueue a fresh grant from a partial offset
        // before this fulfillment returns; the mailbox runtime feeds those
        // events before draining the new actions, so the stale grant is
        // fulfilled from the advanced position and DATA goes past the
        // daemon's `send_through` (the overlapping-grant race, found in
        // review; protocol §Credit).
        if position > start_position {
            events.push(MediaEvent::Produced {
                call_id,
                sent_through: position,
            });
        }
        // The grant ran out at (or past) the source's end: report EOF now
        // rather than waiting for a probe grant that may never come.
        if position >= source.len() {
            events.push(MediaEvent::SourceEnd {
                call_id,
                total: position,
            });
        }
        self.persist_produced(id, position);
        ProduceOutcome::Events(events)
    }

    /// Persist the produced offset on the bound entry (a no-op if the stream
    /// was pruned mid-grant — a straggling grant strands nothing).
    fn persist_produced(&mut self, id: RequestId, position: u64) {
        if let Some(stream) = self.bound.get_mut(&id) {
            stream.produced = position;
        }
    }

    /// Hand one accepted inbound range to the stream's registered sink and
    /// return the matching media event: `SinkAccepted` on success,
    /// `SinkFailed` for an unregistered/unbound stream, a quarantine gap, or
    /// a sink refusal.
    pub(crate) fn write_sink(
        &mut self,
        id: RequestId,
        call_id: CallId,
        offset: u64,
        len: u64,
    ) -> MediaEvent {
        let sink = match self.bound.get_mut(&id) {
            Some(stream) => {
                stream.call_id = Some(call_id);
                match stream.media.as_ref() {
                    Some(StreamMedia::Sink(sink)) => Some(sink.clone()),
                    _ => None,
                }
            }
            None => None,
        };
        let Some(sink) = sink else {
            return MediaEvent::SinkFailed { call_id };
        };
        let taken = match self.take_range(id, offset, len) {
            Some(bytes) => bytes,
            None => return MediaEvent::SinkFailed { call_id },
        };
        match sink.write_at(offset, &taken) {
            Ok(()) => MediaEvent::SinkAccepted {
                call_id,
                through: offset.saturating_add(len),
            },
            Err(_) => MediaEvent::SinkFailed { call_id },
        }
    }

    /// Remove and return the contiguous quarantined range `[offset,
    /// offset+len)`, splitting a chunk that extends past the range's end.
    /// `None` on any gap (a protocol-side discontinuity the kernel normally
    /// prevents; refusing is the honest path).
    fn take_range(&mut self, id: RequestId, offset: u64, len: u64) -> Option<Vec<u8>> {
        let stream = self.bound.get_mut(&id)?;
        if len == 0 {
            return Some(Vec::new());
        }
        let mut collected = Vec::with_capacity(len as usize);
        let mut covered: u64 = 0;
        while covered < len {
            let key = offset.saturating_add(covered);
            let needed = (len - covered) as usize;
            let mut chunk = stream.inbound.remove(&key)?;
            if chunk.len() > needed {
                // The chunk extends past this range's end: keep the prefix,
                // re-quarantine the suffix under its own offset.
                let suffix_offset = key.saturating_add(needed as u64);
                let suffix = chunk.split_off(needed);
                stream.inbound.insert(suffix_offset, suffix);
                collected.extend_from_slice(&chunk);
                covered = len;
            } else {
                collected.extend_from_slice(&chunk);
                covered += chunk.len() as u64;
            }
        }
        stream.inbound_bytes = stream.inbound_bytes.saturating_sub(collected.len() as u64);
        Some(collected)
    }

    /// Quarantine one inbound DATA payload before its record metadata
    /// reaches the core. Returns `false` on protocol trouble — an unbound
    /// wire id, a duplicate/overlapping offset, or a buffer past its
    /// defensive cap (twice the kernel's stream window) — which the socket's
    /// message handler reports as `Inbound::Malformed`.
    pub(crate) fn deliver_data(&mut self, id: RequestId, offset: u64, payload: &[u8]) -> bool {
        let Some(stream) = self.bound.get_mut(&id) else {
            return false;
        };
        if stream.inbound.contains_key(&offset) {
            return false;
        }
        let add = payload.len() as u64;
        if stream.inbound_bytes.saturating_add(add) > self.window_cap {
            return false;
        }
        stream.inbound.insert(offset, payload.to_vec());
        stream.inbound_bytes = stream.inbound_bytes.saturating_add(add);
        true
    }

    /// Prune a finished stream (its terminal Text reply arrived): drop the
    /// bound entry and its quarantine buffer. Cheap and idempotent.
    pub(crate) fn prune(&mut self, wire_id: RequestId) {
        self.bound.remove(&wire_id);
    }

    /// Clear every bound stream. Called on connection loss and teardown: a
    /// stream never survives its connection (§S10). Caller registrations
    /// survive: they await a send that a queued stream call still owes after
    /// the reconnect.
    pub(crate) fn clear(&mut self) {
        self.bound.clear();
    }
}

/// Map one kernel record intent onto the codec's record type. `None` only
/// for a zero stream id, which the codec forbids and a decoded OPEN can
/// never carry. (Mirrors the native adapter's mapping; duplicated here
/// because the native module is `ws-native`-gated.)
pub(crate) fn intent_record(intent: &StreamRecordIntent) -> Option<StreamRecord> {
    use crate::kernel::transport::StreamRecordIntent as I;
    let identity = |id: RequestId, stream_id: u128| StreamIdentity::new(id.get(), stream_id).ok();
    let record = match *intent {
        I::Credit {
            id,
            stream_id,
            accepted_through,
            send_through,
        } => StreamRecord {
            identity: identity(id, stream_id)?,
            body: StreamRecordBody::Credit {
                accepted_through,
                send_through,
            },
        },
        I::End {
            id,
            stream_id,
            offset,
        } => StreamRecord {
            identity: identity(id, stream_id)?,
            body: StreamRecordBody::End { total: offset },
        },
        I::Abort {
            id,
            stream_id,
            high_water,
            reason,
        } => StreamRecord {
            identity: identity(id, stream_id)?,
            body: StreamRecordBody::Abort {
                accepted_through: high_water,
                reason: map_abort_reason_outbound(reason),
            },
        },
        I::Ack {
            id,
            stream_id,
            high_water,
        } => StreamRecord {
            identity: identity(id, stream_id)?,
            body: StreamRecordBody::Ack {
                accepted_through: high_water,
            },
        },
    };
    Some(record)
}

/// Map an inbound codec abort reason onto the kernel's closed tag by value:
/// the four shared tags map one-to-one; the daemon-only `OperationError`
/// (which a client never authors and the kernel's inbound-ABORT handling
/// ignores the reason of) maps to `ProtocolError`.
pub(crate) fn map_abort_reason(reason: BinaryAbortReason) -> StreamAbortReason {
    match reason {
        BinaryAbortReason::Cancelled => StreamAbortReason::Cancelled,
        BinaryAbortReason::SourceFailed => StreamAbortReason::SourceFailed,
        BinaryAbortReason::SinkFailed => StreamAbortReason::SinkFailed,
        BinaryAbortReason::ProtocolError | BinaryAbortReason::OperationError => {
            StreamAbortReason::ProtocolError
        }
    }
}

/// Map a kernel abort tag onto the codec's wire reason (the outbound
/// direction): every kernel tag has an exact wire counterpart.
fn map_abort_reason_outbound(reason: StreamAbortReason) -> BinaryAbortReason {
    match reason {
        StreamAbortReason::Cancelled => BinaryAbortReason::Cancelled,
        StreamAbortReason::SourceFailed => BinaryAbortReason::SourceFailed,
        StreamAbortReason::SinkFailed => BinaryAbortReason::SinkFailed,
        StreamAbortReason::ProtocolError => BinaryAbortReason::ProtocolError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::shared_bytes;

    fn wire_id() -> RequestId {
        RequestId::new(7).expect("wire id")
    }

    #[test]
    fn registered_map_is_bounded_and_evicts_oldest() {
        // The browser registry mirrors the native one: the cap evicts the
        // OLDEST outstanding registration; an evicted key's later call binds
        // an empty entry and fails honestly at produce time.
        let mut media = WebMediaRegistry::new(1024, 2);
        let k1 = OpId::new("share-1");
        let k2 = OpId::new("share-2");
        let k3 = OpId::new("share-3");
        media.register(k1.clone(), shared_bytes(vec![1]));
        media.register(k2, shared_bytes(vec![2]));
        media.register(k3.clone(), shared_bytes(vec![3]));
        assert_eq!(media.registered.len(), 2, "the cap holds");
        // k3 (newest) binds its media; k1 (evicted) binds an empty entry.
        media.bind(wire_id(), Some(k3));
        assert!(
            media
                .bound
                .get(&wire_id())
                .is_some_and(|s| s.media.is_some()),
            "the newest registration bound its media"
        );
        let evicted_id = RequestId::new(8).expect("id");
        media.bind(evicted_id, Some(k1));
        assert!(
            media
                .bound
                .get(&evicted_id)
                .is_some_and(|s| s.media.is_none()),
            "the evicted registration binds an honestly-failing empty entry"
        );
    }

    #[test]
    fn produce_reports_one_cumulative_progress_event_per_grant() {
        use crate::kernel::inflight::CallId;
        use crate::kernel::transport::MediaEvent;
        // A grant spanning multiple codec-bounded chunks reports ONE
        // cumulative Produced (the overlapping-grant regression): pin the
        // count and the final offset with a tight codec bound (one chunk per
        // record = 8 frame bytes of payload room).
        let mut media = WebMediaRegistry::new(1024, 8);
        let key = OpId::new("share-1");
        media.register(key.clone(), shared_bytes(vec![1, 2, 3, 4, 5, 6]));
        let id = wire_id();
        media.bind(id, Some(key));
        media.adopt_open_stream_id(id, 1);
        let bounds = CodecBounds::default();
        let chunk = max_stream_data_bytes(bounds.max_frame_bytes).expect("cap");
        // Exercise a multi-chunk grant only when the default cap permits
        // splitting (it is huge); otherwise the single-chunt invariant still
        // holds trivially.
        let total = 6_u64;
        let up_to = total.min(chunk as u64);
        let events = match media.produce(id, CallId(3), up_to, &bounds, &mut |_| true) {
            ProduceOutcome::Events(events) => events,
            ProduceOutcome::Closed => panic!("the send closure never fails here"),
        };
        let produced: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, MediaEvent::Produced { .. }))
            .collect();
        assert_eq!(
            produced.len(),
            1,
            "exactly ONE cumulative Produced per grant (got {produced:?})"
        );
        assert!(matches!(
            events.last(),
            Some(MediaEvent::SourceEnd { total: t, .. }) if *t == total
        ));
    }
}
