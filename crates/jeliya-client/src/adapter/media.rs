//! The native adapter's byte-stream media drive (§S3 of the kernel stream
//! spec): the registry that binds registered [`StreamMedia`] to wire ids and
//! moves real bytes between the caller's sources/sinks and the socket.
//!
//! The kernel's stream control plane is byte-free — it grants offsets and
//! windows, never payload. This module is where those grants become
//! `JBS2` DATA records on the write channel (one async media task per active
//! producer stream, awaiting the writer's backpressure) and where inbound
//! DATA payloads quarantine (window-bounded) until the kernel's `WriteSink`
//! hands them to the caller's sink. All framing stays in `jeliya-codec`,
//! called only from here and the read loop — no `JBS2` constant appears in
//! this file.
//!
//! Honest failure is the invariant: a stream that reaches a media effect
//! with no registered media reports `SourceFailed`/`SinkFailed` to the core
//! (which aborts the stream and settles the call), never a stall and never a
//! fake success. A stream never survives its connection: teardown clears the
//! bound entries and aborts their media tasks exactly when the connection's
//! tasks die (§S10).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Weak};

use jeliya_api::{OpId, RequestId};
use jeliya_codec::{
    max_stream_data_bytes, BinaryAbortReason, CodecBounds, StreamIdentity, StreamRecord,
    StreamRecordBody,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::kernel::core::Input;
use crate::kernel::inflight::CallId;
use crate::kernel::transport::{StreamAbortReason, StreamRecordIntent};
use crate::media::{ByteSource, StreamMedia};

/// One producer grant forwarded to a stream's media task: how many more
/// bytes the kernel has bounded (credit + window + total) and which call the
/// resulting `Produced`/`SourceEnd`/`SourceFailed` inputs belong to. The
/// call id rides here because binding happens at send time, when only the
/// wire id is known; [`MediaRegistry::produce`] records it on the bound
/// entry and passes it through the channel in the same step.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MediaGrant {
    /// The stream call the grant fulfils.
    pub(crate) call_id: CallId,
    /// The maximum additional bytes the task may send now.
    pub(crate) up_to: u64,
}

/// The media-side state of one bound stream (keyed by wire id): the caller's
/// registered media, the producer task's grant channel, and the receiver's
/// inbound quarantine buffer.
///
/// `Debug` is deliberately absent as a derive: the inbound buffer holds
/// payload bytes, and no debug render of this type may expose them (§S12).
pub(crate) struct BoundStream {
    /// The registered media, if the caller registered any (`None` = an
    /// honestly-failing unregistered stream).
    pub(crate) media: Option<StreamMedia>,
    /// The producer media task's grant sender, present from OPEN on a
    /// source-bound stream.
    pub(crate) grant_tx: Option<mpsc::Sender<MediaGrant>>,
    /// The producer media task, if one was spawned.
    pub(crate) task: Option<JoinHandle<()>>,
    /// The call id observed at the first media effect (`produce`/`write_sink`
    /// both receive it; the producer task also carries it per grant).
    pub(crate) call_id: Option<CallId>,
    /// Quarantined inbound DATA, keyed by offset — bounded by the registry's
    /// defensive cap (twice the kernel's stream window).
    pub(crate) inbound: BTreeMap<u64, Vec<u8>>,
    /// The buffer's running byte count (the cap check's numerator).
    pub(crate) inbound_bytes: u64,
}

impl BoundStream {
    /// A fresh, media-less bound entry.
    fn empty() -> Self {
        Self {
            media: None,
            grant_tx: None,
            task: None,
            call_id: None,
            inbound: BTreeMap::new(),
            inbound_bytes: 0,
        }
    }
}

/// The per-connection media registry, shared (inside the connection
/// registry) between the native driver, the dial task's read loop, and every
/// teardown path.
///
/// Two maps, two lifetimes: `registered` holds caller registrations keyed by
/// dedup `OpId` awaiting the stream op's send; `bound` holds the per-stream
/// state keyed by wire id from that send until the stream's terminal reply
/// ([`MediaRegistry::prune`]) or the connection's death
/// ([`MediaRegistry::clear`]). Registrations survive a connection loss —
/// a stream call queued across a reconnect still sends after it, and binds
/// then; bound streams never do (§S8/§S10).
pub(crate) struct MediaRegistry {
    /// Caller registrations awaiting a stream op's send, keyed by dedup
    /// `OpId`. Bounded by the caller's own registration discipline.
    registered: HashMap<OpId, StreamMedia>,
    /// Per-stream media state, keyed by the stream's wire id. Bounded by the
    /// kernel's concurrent-stream limit (every terminal prunes).
    bound: HashMap<RequestId, BoundStream>,
    /// The defensive inbound-buffer cap: the kernel's `stream_window_bytes`
    /// doubled (the core grants at most one window; the second window is the
    /// margin for a range delivered just before its credit extension).
    window_cap: u64,
    /// Media inputs fulfilled synchronously (`SinkAccepted`/`SinkFailed`,
    /// and `SourceFailed` for an unregistered stream), drained by the shell's
    /// `take_pending_media` loop after the apply batch — the driver never
    /// injects while holding the registry lock.
    pending: VecDeque<Input>,
}

impl MediaRegistry {
    /// Build an empty registry whose inbound cap is `stream_window_bytes × 2`
    /// (the kernel config's window, passed at construction).
    pub(crate) fn new(stream_window_bytes: u64) -> Self {
        Self {
            registered: HashMap::new(),
            bound: HashMap::new(),
            window_cap: stream_window_bytes.saturating_mul(2),
            pending: VecDeque::new(),
        }
    }

    /// Register one stream's media under its dedup key, before the call is
    /// dispatched. Re-registering a key replaces the previous media (the
    /// caller's own last-write-wins).
    pub(crate) fn register(&mut self, key: OpId, media: StreamMedia) {
        self.registered.insert(key, media);
    }

    /// Bind a stream op's wire id at send time: move the caller's
    /// registration (if any) onto the wire key. A stream op with **no**
    /// registration still binds an empty entry, so its later media effects
    /// fail honestly (`SourceFailed`/`SinkFailed`) instead of silently. A
    /// re-send of an already-bound id (a reconnect's replay of the send)
    /// leaves the existing binding untouched.
    pub(crate) fn bind(&mut self, wire_id: RequestId, op_id: Option<OpId>) {
        if self.bound.contains_key(&wire_id) {
            return;
        }
        let media = op_id.and_then(|key| self.registered.remove(&key));
        let mut stream = BoundStream::empty();
        stream.media = media;
        self.bound.insert(wire_id, stream);
    }

    /// Whether a wire id has a bound stream entry.
    pub(crate) fn is_bound(&self, wire_id: RequestId) -> bool {
        self.bound.contains_key(&wire_id)
    }

    /// Frame one outbound control record via `jeliya-codec` and push it onto
    /// the live write channel (non-blocking, exactly like a Text send).
    /// Returns `false` when the record cannot be framed or the channel is
    /// full/closed — the caller treats that exactly like a failed Text send
    /// (a connection loss the core reclassifies).
    pub(crate) fn send_record(writer: &mpsc::Sender<Message>, intent: &StreamRecordIntent) -> bool {
        let record = match intent_record(intent) {
            Some(record) => record,
            None => return false,
        };
        // Control records carry no payload, so the codec's own default frame
        // ceiling is always satisfiable; a served limit cannot be tighter
        // than the 48-byte header a negotiated connection already carried.
        let framed = match encode_control(&record) {
            Some(bytes) => bytes,
            None => return false,
        };
        writer.try_send(Message::Binary(framed.into())).is_ok()
    }

    /// Fulfil one `ProduceData` grant: record the call id on the bound entry
    /// and forward the grant to the stream's media task. A stream with no
    /// media task (unregistered, or a sink-bound stream) queues an honest
    /// `SourceFailed` for the shell to re-drive — never a silent drop.
    pub(crate) fn produce(&mut self, id: RequestId, call_id: CallId, up_to: u64) {
        let grant_tx = match self.bound.get_mut(&id) {
            Some(stream) => {
                stream.call_id = Some(call_id);
                stream.grant_tx.clone()
            }
            None => None,
        };
        match grant_tx {
            // A closed/full channel means the task ended (source already
            // reported its terminal) or is maximally backed up; the kernel
            // already holds the source's terminal or will re-pump on the
            // next credit — dropping the grant strands nothing.
            Some(tx) => {
                let _ = tx.try_send(MediaGrant { call_id, up_to });
            }
            None => self.pending.push_back(Input::SourceFailed { call_id }),
        }
    }

    /// Fulfil one `WriteSink` hand-off: take the quarantined range, write it
    /// to the caller's sink, and queue the matching input for the shell. A
    /// gap in the quarantine or a sink refusal queues `SinkFailed`.
    pub(crate) fn write_sink(&mut self, id: RequestId, call_id: CallId, offset: u64, len: u64) {
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
            self.pending.push_back(Input::SinkFailed { call_id });
            return;
        };
        let taken = match self.take_range(id, offset, len) {
            Some(bytes) => bytes,
            None => {
                self.pending.push_back(Input::SinkFailed { call_id });
                return;
            }
        };
        match sink.write_at(offset, &taken) {
            Ok(()) => self.pending.push_back(Input::SinkAccepted {
                call_id,
                through: offset.saturating_add(len),
            }),
            Err(_) => self.pending.push_back(Input::SinkFailed { call_id }),
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
    /// defensive cap (twice the kernel's stream window) — which the read
    /// loop reports as `Inbound::Malformed`.
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

    /// Spawn (once, at OPEN) the producer media task for a source-bound
    /// stream: it owns the grant channel and the caller's byte source, reads
    /// granted bytes in codec-bounded chunks, frames DATA records, and
    /// awaits the writer's backpressure — reporting progress and terminals
    /// through the shell's `inject`. No-op when the stream has no source
    /// media or already has a task.
    pub(crate) fn spawn_source_task(
        &mut self,
        wire_id: RequestId,
        stream_id: u128,
        writer: &mpsc::Sender<Message>,
        runtime: Weak<crate::kernel::Runtime>,
        bounds: CodecBounds,
    ) {
        let stream = match self.bound.get_mut(&wire_id) {
            Some(stream) if stream.grant_tx.is_none() => stream,
            _ => return,
        };
        let source = match stream.media.as_ref() {
            Some(StreamMedia::Source(source)) => source.clone(),
            _ => return,
        };
        let identity = match StreamIdentity::new(wire_id.get(), stream_id) {
            Ok(identity) => identity,
            Err(_) => return,
        };
        let (grant_tx, grant_rx) = mpsc::channel::<MediaGrant>(8);
        let task = tokio::spawn(source_task(
            runtime,
            writer.clone(),
            identity,
            source,
            grant_rx,
            bounds,
        ));
        stream.grant_tx = Some(grant_tx);
        stream.task = Some(task);
    }

    /// Prune a finished stream (its terminal Text reply arrived): abort any
    /// media task and drop the bound entry. Cheap and idempotent.
    pub(crate) fn prune(&mut self, wire_id: RequestId) {
        if let Some(mut stream) = self.bound.remove(&wire_id) {
            if let Some(task) = stream.task.take() {
                task.abort();
            }
        }
    }

    /// Clear every bound stream — abort media tasks, drop quarantine buffers.
    /// Called on connection loss and teardown: a stream never survives its
    /// connection (§S10). Caller registrations survive: they await a send
    /// that a queued stream call still owes after the reconnect.
    pub(crate) fn clear(&mut self) {
        for (_, mut stream) in self.bound.drain() {
            if let Some(task) = stream.task.take() {
                task.abort();
            }
        }
    }

    /// Drain the synchronously-fulfilled media inputs for the shell's
    /// post-batch re-drive (mirrors the in-memory driver's queue).
    pub(crate) fn take_pending(&mut self) -> VecDeque<Input> {
        std::mem::take(&mut self.pending)
    }

    /// The number of bound streams (test observability; a bound, never
    /// content).
    #[cfg(test)]
    pub(crate) fn bound_len(&self) -> usize {
        self.bound.len()
    }
}

/// Encode one payload-free control record under the codec's default bounds.
/// Control records are exactly the 48-byte header, so any negotiated limit
/// that carried the stream's OPEN carries these too.
fn encode_control(record: &StreamRecord) -> Option<Vec<u8>> {
    jeliya_codec::encode_stream_record(record, &CodecBounds::default()).ok()
}

/// Map one kernel record intent onto the codec's record type. `None` only
/// for a zero stream id, which the codec forbids and a decoded OPEN can
/// never carry.
fn intent_record(intent: &StreamRecordIntent) -> Option<StreamRecord> {
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

/// The producer media task: one per active source-bound stream. Reads
/// granted bytes from the caller's source in codec-bounded chunks, frames
/// DATA records, sends them through the writer (awaiting its backpressure —
/// this task holds no locks), and injects `Produced`/`SourceEnd`/
/// `SourceFailed` through the shell. A writer send failure ends the task
/// quietly: the read loop observes the connection loss and the core tears
/// every stream down (§S10).
async fn source_task(
    runtime: Weak<crate::kernel::Runtime>,
    writer: mpsc::Sender<Message>,
    identity: StreamIdentity,
    source: Arc<dyn ByteSource>,
    mut grants: mpsc::Receiver<MediaGrant>,
    bounds: CodecBounds,
) {
    let chunk_cap = match max_stream_data_bytes(bounds.max_frame_bytes) {
        Ok(cap) => cap,
        Err(_) => return,
    };
    let inject = |input: Input| {
        if let Some(runtime) = runtime.upgrade() {
            runtime.inject(input);
        }
    };
    let mut position: u64 = 0;
    while let Some(grant) = grants.recv().await {
        let mut remaining = grant.up_to;
        while remaining > 0 {
            let want = remaining.min(chunk_cap as u64) as usize;
            let mut buf = vec![0_u8; want];
            let read = source.read_at(position, &mut buf);
            if read == 0 {
                // EOF reports the source's own count (the authoritative END
                // offset); a zero read short of it is a source failure.
                if position >= source.len() {
                    inject(Input::SourceEnd {
                        call_id: grant.call_id,
                        total: position,
                    });
                } else {
                    inject(Input::SourceFailed {
                        call_id: grant.call_id,
                    });
                }
                return;
            }
            buf.truncate(read);
            let record = StreamRecord {
                identity,
                body: StreamRecordBody::Data {
                    offset: position,
                    payload: buf,
                },
            };
            let framed = match jeliya_codec::encode_stream_record(&record, &bounds) {
                Ok(bytes) => bytes,
                // A framing refusal is a local media failure: abort honestly.
                Err(_) => {
                    inject(Input::SourceFailed {
                        call_id: grant.call_id,
                    });
                    return;
                }
            };
            if writer.send(Message::Binary(framed.into())).await.is_err() {
                // Connection loss: the read loop reports it; nothing here can
                // or should continue.
                return;
            }
            position += read as u64;
            remaining = remaining.saturating_sub(read as u64);
            inject(Input::Produced {
                call_id: grant.call_id,
                sent_through: position,
            });
        }
        // The grant ran out at (or past) the source's end: report EOF now
        // rather than waiting for a probe grant that may never come.
        if position >= source.len() {
            inject(Input::SourceEnd {
                call_id: grant.call_id,
                total: position,
            });
            return;
        }
    }
}

/// Resize-and-take note: the read buffer is truncated to the source's short
/// read before framing, so no per-chunk re-allocation happens (the codec
/// copies the payload into the framed record).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{shared_bytes, ByteSink, SinkRejected};
    use jeliya_api::OpId;

    fn wire_id() -> RequestId {
        RequestId::new(7).expect("wire id")
    }

    fn registry() -> MediaRegistry {
        MediaRegistry::new(1024)
    }

    #[test]
    fn bind_moves_a_registration_and_leaves_it_consumed() {
        let mut media = registry();
        let key = OpId::new("share-1");
        media.register(key.clone(), shared_bytes(vec![1, 2, 3]));
        media.bind(wire_id(), Some(key.clone()));
        assert!(media.is_bound(wire_id()));
        // The registration moved: the same key binds nothing the second time.
        media.bind(RequestId::new(8).expect("id"), Some(key));
        assert!(media.is_bound(RequestId::new(8).expect("id")));
        // A stream op with no registration still binds an empty entry.
        media.bind(RequestId::new(9).expect("id"), None);
        assert!(media.is_bound(RequestId::new(9).expect("id")));
        // Bound state is per wire id: 3 bound streams here.
        assert_eq!(media.bound_len(), 3);
    }

    #[test]
    fn unregistered_stream_produce_fails_honestly() {
        let mut media = registry();
        let id = wire_id();
        media.bind(id, None);
        let call_id = CallId(11);
        media.produce(id, call_id, 64);
        let pending = media.take_pending();
        assert!(
            matches!(pending.front(), Some(Input::SourceFailed { call_id: c }) if *c == call_id),
            "an unregistered stream must report SourceFailed (queue len {})",
            pending.len()
        );
        assert!(media.take_pending().is_empty(), "the queue drains once");
    }

    #[test]
    fn unbound_wire_id_produce_also_fails_honestly() {
        let mut media = registry();
        let call_id = CallId(4);
        media.produce(wire_id(), call_id, 8);
        assert!(
            matches!(
                media.take_pending().front(),
                Some(Input::SourceFailed { .. })
            ),
            "a produce for a never-bound wire id strands nothing and fails honestly"
        );
    }

    #[test]
    fn deliver_data_enforces_the_window_cap() {
        let mut media = registry();
        let id = wire_id();
        media.bind(id, None);
        // Cap = 2 × 1024 = 2048 bytes.
        assert!(media.deliver_data(id, 0, &vec![0_u8; 1024]));
        assert!(media.deliver_data(id, 1024, &vec![0_u8; 1024]));
        // One more byte crosses twice the kernel window: refused.
        assert!(!media.deliver_data(id, 2048, &[9]));
        // A duplicate offset is protocol trouble: refused.
        assert!(!media.deliver_data(id, 0, &[1]));
        // An unbound wire id is refused.
        assert!(!media.deliver_data(RequestId::new(99).expect("id"), 0, &[1]));
    }

    #[test]
    fn sink_write_takes_the_buffered_range_and_reports_accepted() {
        let mut media = registry();
        let id = wire_id();
        let sink = crate::media::CollectedBytes::new();
        let mut stream = BoundStream::empty();
        stream.media = Some(crate::media::StreamMedia::Sink(std::sync::Arc::new(sink)));
        media.bound.insert(id, stream);
        assert!(media.deliver_data(id, 0, &[1, 2, 3]));
        let call_id = CallId(21);
        media.write_sink(id, call_id, 0, 3);
        let pending = media.take_pending();
        assert!(
            matches!(pending.front(), Some(Input::SinkAccepted { call_id: c, through: 3 }) if *c == call_id),
            "an accepted range reports SinkAccepted through its end"
        );
        // The quarantined bytes were consumed: re-writing the same range now
        // hits a gap (nothing is double-written).
        media.write_sink(id, call_id, 0, 3);
        assert!(
            matches!(media.take_pending().front(), Some(Input::SinkFailed { .. })),
            "a consumed range must not be writable twice"
        );
    }

    #[test]
    fn sink_gap_refusal_reports_sink_failed() {
        let mut media = registry();
        let id = wire_id();
        let mut stream = BoundStream::empty();
        stream.media = Some(crate::media::StreamMedia::Sink(std::sync::Arc::new(
            crate::media::CollectedBytes::new(),
        )));
        media.bound.insert(id, stream);
        // Nothing was delivered at 0: writing [0, 3) hits a gap.
        let call_id = CallId(31);
        media.write_sink(id, call_id, 0, 3);
        assert!(
            matches!(media.take_pending().front(), Some(Input::SinkFailed { call_id: c }) if *c == call_id),
            "a quarantine gap must report SinkFailed, never fake acceptance"
        );
    }

    #[test]
    fn sink_rejection_reports_sink_failed() {
        struct Refusing;
        impl ByteSink for Refusing {
            fn write_at(&self, _offset: u64, _bytes: &[u8]) -> Result<(), SinkRejected> {
                Err(SinkRejected)
            }
        }
        let mut media = registry();
        let id = wire_id();
        let mut stream = BoundStream::empty();
        stream.media = Some(crate::media::StreamMedia::Sink(std::sync::Arc::new(
            Refusing,
        )));
        media.bound.insert(id, stream);
        assert!(media.deliver_data(id, 0, &[1]));
        let call_id = CallId(41);
        media.write_sink(id, call_id, 0, 1);
        assert!(
            matches!(media.take_pending().front(), Some(Input::SinkFailed { .. })),
            "a refusing sink must report SinkFailed"
        );
    }

    #[test]
    fn prune_removes_the_bound_stream_and_is_idempotent() {
        let mut media = registry();
        let id = wire_id();
        media.bind(id, None);
        media.prune(id);
        assert_eq!(media.bound_len(), 0);
        media.prune(id); // no panic, no effect
    }

    #[test]
    fn clear_drops_bound_streams_but_keeps_registrations() {
        let mut media = registry();
        let key = OpId::new("queued");
        media.register(key.clone(), shared_bytes(vec![1]));
        media.bind(wire_id(), None);
        media.clear();
        assert_eq!(media.bound_len(), 0, "no stream survives its connection");
        // The registration still awaits its send across the reconnect.
        media.bind(RequestId::new(50).expect("id"), Some(key));
        assert_eq!(media.bound_len(), 1);
    }

    #[test]
    fn abort_reason_mapping_is_by_closed_tag() {
        assert_eq!(
            map_abort_reason(BinaryAbortReason::Cancelled),
            StreamAbortReason::Cancelled
        );
        assert_eq!(
            map_abort_reason(BinaryAbortReason::SourceFailed),
            StreamAbortReason::SourceFailed
        );
        assert_eq!(
            map_abort_reason(BinaryAbortReason::SinkFailed),
            StreamAbortReason::SinkFailed
        );
        assert_eq!(
            map_abort_reason(BinaryAbortReason::ProtocolError),
            StreamAbortReason::ProtocolError
        );
        // Daemon-only on the wire; a client maps it to the protocol tag.
        assert_eq!(
            map_abort_reason(BinaryAbortReason::OperationError),
            StreamAbortReason::ProtocolError
        );
        // The outbound direction is exact for every kernel tag.
        assert_eq!(
            map_abort_reason_outbound(StreamAbortReason::Cancelled),
            BinaryAbortReason::Cancelled
        );
        assert_eq!(
            map_abort_reason_outbound(StreamAbortReason::SourceFailed),
            BinaryAbortReason::SourceFailed
        );
        assert_eq!(
            map_abort_reason_outbound(StreamAbortReason::SinkFailed),
            BinaryAbortReason::SinkFailed
        );
        assert_eq!(
            map_abort_reason_outbound(StreamAbortReason::ProtocolError),
            BinaryAbortReason::ProtocolError
        );
    }

    #[test]
    fn send_record_frames_a_credit_record_onto_the_channel() {
        let (tx, mut rx) = mpsc::channel::<Message>(2);
        let intent = StreamRecordIntent::Credit {
            id: wire_id(),
            stream_id: 1,
            accepted_through: 0,
            send_through: 10,
        };
        assert!(MediaRegistry::send_record(&tx, &intent));
        match rx.try_recv() {
            Ok(Message::Binary(_)) => {}
            other => panic!("expected a framed Binary record, got {other:?}"),
        }
        // A zero stream id cannot be framed: honest failure.
        let bad = StreamRecordIntent::End {
            id: wire_id(),
            stream_id: 0,
            offset: 0,
        };
        assert!(!MediaRegistry::send_record(&tx, &bad));
    }

    #[test]
    fn send_record_fails_on_a_closed_channel() {
        let (tx, _rx) = mpsc::channel::<Message>(1);
        drop(_rx);
        let intent = StreamRecordIntent::Ack {
            id: wire_id(),
            stream_id: 3,
            high_water: 7,
        };
        assert!(
            !MediaRegistry::send_record(&tx, &intent),
            "a closed write channel must report failure, not success"
        );
    }
}
