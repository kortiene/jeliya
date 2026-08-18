//! The public stream-media seam: the byte sources and sinks a caller
//! registers for one duplex byte-stream operation (`file.share` uploads bytes
//! from a [`ByteSource`]; `file.read` downloads bytes into a [`ByteSink`]).
//!
//! The kernel's stream control plane (#269) is byte-free by construction — it
//! grants offsets and windows, never touches payload bytes (§S2 of
//! `specs/rust-client-kernel-stream-lifecycle.md`). The **driver** moves the
//! bytes, and this module is the caller-owned media it moves them from/to.
//! Registration reaches the driver through
//! [`ClientHandle::register_stream_media`](crate::ClientHandle::register_stream_media)
//! keyed by the operation's dedup [`OpId`](jeliya_api::OpId); the driver binds
//! that key to the stream's wire id when it performs the request's send. A
//! stream that reaches its media effect with no registration fails honestly
//! (`source_failed`/`sink_failed`), never a stall or a fake success.
//!
//! `PlatformServices`-backed file media (#174) plugs in through the same
//! traits with no adapter change — the adapters depend on the traits, not the
//! in-memory types.

use std::sync::Arc;

use jeliya_api::OpId;

/// A producer's byte source for one `file.share` stream.
///
/// The driver calls [`read_at`](ByteSource::read_at) with offsets the kernel
/// has already bounded by credit and the declared total, so a source never
/// hands out bytes the daemon has not admitted. Implementations must be
/// `Send + Sync` (the driver may fulfil grants from its own tasks).
pub trait ByteSource: Send + Sync {
    /// Copy up to `buf.len()` bytes starting at `offset` into `buf`, returning
    /// how many were copied. `offset` is always `< len()`; a short read is
    /// legal (the driver simply sends what it got and reports the new
    /// high-water), and `0` at a non-EOF position is a source failure.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> usize;
    /// The source's total byte count. Must equal the request's
    /// `declared_bytes` — the daemon checks the pair at admission.
    fn len(&self) -> u64;
    /// Standard predicate; a `len() == 0` source drives the protocol's
    /// zero-byte handshake.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An empty payload marks "the sink cannot accept" — the stream aborts
/// `sink_failed` honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkRejected;

impl std::fmt::Display for SinkRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the sink refused the delivered range")
    }
}

impl std::error::Error for SinkRejected {}

/// A receiver's byte sink for one `file.read` stream.
///
/// The driver calls [`write_at`](ByteSink::write_at) with ranges the kernel
/// has validated as contiguous from the accepted high-water and within the
/// granted credit window. Downloaded bytes are quarantined by the kernel until
/// END and the success reply agree on the count; the sink is where they land.
pub trait ByteSink: Send + Sync {
    /// Accept `bytes` at `offset`. Returns [`SinkRejected`] if the sink cannot
    /// accept (disk full, caller dropped the collection) — the driver reports
    /// `sink_failed` and the stream aborts honestly.
    fn write_at(&self, offset: u64, bytes: &[u8]) -> Result<(), SinkRejected>;
}

/// One caller-owned in-memory [`ByteSource`]: a shared, immutable byte slice.
///
/// The caller keeps the bytes anyway (a picker result, a generated blob); the
/// source is a window over them, so no copy is forced. Bounded by the caller's
/// own allocation, itself bounded by the daemon's `max_shared_file_bytes`.
#[derive(Clone)]
pub struct SharedBytes {
    bytes: Arc<[u8]>,
}

impl SharedBytes {
    /// Wrap `bytes` as a stream source.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    /// The underlying slice (for a caller-side digest, a preview, …).
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

impl ByteSource for SharedBytes {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> usize {
        let start = offset as usize;
        if self.bytes.len() <= start {
            return 0;
        }
        let end = (start + buf.len()).min(self.bytes.len());
        let count = end - start;
        buf[..count].copy_from_slice(&self.bytes[start..end]);
        count
    }

    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// One caller-owned in-memory [`ByteSink`]: bytes collected contiguously and
/// taken by the caller after the stream's terminal reply.
///
/// Writes must arrive contiguously from zero (the kernel guarantees it); a
/// discontinuity or an overwrite is refused — the sink never silently drops or
/// double-writes a byte.
#[derive(Clone, Default)]
pub struct CollectedBytes {
    inner: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl CollectedBytes {
    /// A fresh, empty collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Take everything collected so far, leaving the sink empty. Safe to call
    /// after the stream's terminal (the kernel quarantined the bytes until END
    /// and the reply agreed) or after a failure (a partial, honestly
    /// incomplete collection).
    pub fn take(&self) -> Vec<u8> {
        std::mem::take(&mut self.inner.lock().expect("CollectedBytes poisoned"))
    }

    /// How many bytes are collected so far.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("CollectedBytes poisoned").len()
    }

    /// Whether nothing has been collected.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ByteSink for CollectedBytes {
    fn write_at(&self, offset: u64, bytes: &[u8]) -> Result<(), SinkRejected> {
        let mut collected = self.inner.lock().expect("CollectedBytes poisoned");
        let offset = offset as usize;
        if offset != collected.len() {
            // The kernel delivers contiguous-from-accepted ranges only; this
            // cannot happen through a conforming driver, and a gap must never
            // be papered over silently.
            return Err(SinkRejected);
        }
        collected.extend_from_slice(bytes);
        Ok(())
    }
}

/// The media one stream call is bound to: a producer's source or a receiver's
/// sink, registered under the call's dedup `OpId` before dispatch.
///
/// `Debug` is deliberately not payload-rendering: the enum carries only its
/// role tag and the media's byte length.
pub enum StreamMedia {
    /// The producer's source (`file.share`).
    Source(Arc<dyn ByteSource>),
    /// The receiver's sink (`file.read`).
    Sink(Arc<dyn ByteSink>),
}

impl StreamMedia {
    /// The media's byte count (a source's total, or a sink's collected
    /// length) — a bound, never content.
    pub fn byte_len(&self) -> u64 {
        match self {
            StreamMedia::Source(source) => source.len(),
            StreamMedia::Sink(_) => 0,
        }
    }
}

impl std::fmt::Debug for StreamMedia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamMedia::Source(source) => f
                .debug_struct("StreamMedia::Source")
                .field("total_bytes", &source.len())
                .finish(),
            StreamMedia::Sink(_) => f.debug_struct("StreamMedia::Sink").finish_non_exhaustive(),
        }
    }
}

/// Convenience: wrap a [`SharedBytes`] as [`StreamMedia::Source`].
pub fn shared_bytes(bytes: Vec<u8>) -> StreamMedia {
    StreamMedia::Source(Arc::new(SharedBytes::new(bytes)))
}

/// Convenience: wrap a [`CollectedBytes`] as [`StreamMedia::Sink`].
pub fn collected_bytes() -> (CollectedBytes, StreamMedia) {
    let sink = CollectedBytes::new();
    (sink.clone(), StreamMedia::Sink(Arc::new(sink)))
}

/// The key a registration is bound by. Re-exported so callers need only this
/// module for the whole registration surface.
pub type MediaKey = OpId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_bytes_reads_bounded_ranges() {
        let source = SharedBytes::new(vec![1u8, 2, 3, 4, 5]);
        assert_eq!(source.len(), 5);
        let mut buf = [0u8; 2];
        assert_eq!(source.read_at(3, &mut buf), 2);
        assert_eq!(buf, [4, 5]);
        // A short read at the tail.
        let mut tail = [0u8; 4];
        assert_eq!(source.read_at(4, &mut tail), 1);
        assert_eq!(tail[0], 5);
        // Past the end reads nothing.
        assert_eq!(source.read_at(5, &mut buf), 0);
    }

    #[test]
    fn collected_bytes_accepts_contiguous_and_refuses_gaps() {
        let sink = CollectedBytes::new();
        sink.write_at(0, &[1, 2]).expect("contiguous");
        sink.write_at(2, &[3]).expect("contiguous");
        assert!(sink.write_at(5, &[9]).is_err(), "a gap is refused");
        assert_eq!(sink.take(), vec![1, 2, 3]);
        assert!(sink.is_empty(), "take empties the collection");
    }

    #[test]
    fn stream_media_debug_renders_no_content() {
        let media = shared_bytes(vec![7, 7, 7]);
        let rendered = format!("{media:?}");
        assert!(rendered.contains("Source"));
        assert!(!rendered.contains('7'));
    }

    #[test]
    fn a_zero_byte_source_reports_empty() {
        let source = SharedBytes::new(Vec::new());
        assert!(source.is_empty());
        assert_eq!(source.read_at(0, &mut [0u8; 4]), 0);
    }
}
