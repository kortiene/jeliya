//! Daemon-wide byte-transfer admission and checked runtime limits.
//!
//! The logical reservation in this module is deliberately separate from the
//! outbound writer's byte permits: it bounds the number and aggregate declared
//! size of admitted transfers, while the writer bound limits bytes actually
//! resident in memory. Reservations are RAII so every task-abort and disconnect
//! path releases capacity without a bespoke cleanup call.

use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use jeliya_api::{ApiError, Envelope, FileId, FileRead, FileShare, Limits, RoomId};
use jeliya_codec::{max_stream_data_bytes, StreamIdentity, STREAM_HEADER_BYTES};
use tokio::sync::Semaphore;
use tokio::time::Instant;

const CONTROL_QUEUE_ALLOWANCE: usize = 32;
// Tokio stores aggregate semaphore capacity in `usize`, but one
// `acquire_many_owned` request is a `u32`. A complete control message consumes
// one acquisition, so readiness must enforce the smaller bound.
const MAX_CONTROL_BYTE_PERMITS: usize = if Semaphore::MAX_PERMITS < u32::MAX as usize {
    Semaphore::MAX_PERMITS
} else {
    u32::MAX as usize
};

const STREAM_ID_FEISTEL_ROUNDS: u8 = 8;
const STREAM_ID_DOMAIN: &[u8] = b"jeliya.protocol-v2.stream-id.feistel";

/// Checked, host-representable limits used by the WebSocket transfer runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimeLimits {
    max_frame_bytes: usize,
    max_data_payload_bytes: usize,
    data_queue_capacity_bytes: usize,
    upload_ingress_capacity_bytes: usize,
    upload_ingress_capacity_messages: usize,
    control_queue_capacity_messages: usize,
    data_queue_capacity_messages: usize,
    max_concurrent_transfers: u64,
    max_transfer_bytes_inflight: u64,
    transfer_connect_allowance_ms: u64,
    transfer_floor_bits_per_second: u64,
    transfer_stall_ms: u64,
    transfer_stall: Duration,
}

impl RuntimeLimits {
    /// Validate the served limits before the daemon announces readiness.
    pub(crate) fn from_served(limits: &Limits) -> Result<Self, RuntimeConfigError> {
        let max_frame_bytes = usize::try_from(limits.max_frame_bytes).map_err(|_| {
            RuntimeConfigError::MaxFrameBytesNotRepresentable {
                max_frame_bytes: limits.max_frame_bytes,
            }
        })?;
        let max_data_payload_bytes = max_stream_data_bytes(max_frame_bytes).map_err(|_| {
            RuntimeConfigError::FrameCannotCarryData {
                max_frame_bytes,
                minimum_bytes: STREAM_HEADER_BYTES + 1,
            }
        })?;

        let minimum_request_bytes = minimum_stream_request_bytes();
        if max_frame_bytes < minimum_request_bytes {
            return Err(RuntimeConfigError::FrameCannotCarryStreamRequest {
                max_frame_bytes,
                minimum_bytes: minimum_request_bytes,
            });
        }
        if max_frame_bytes > MAX_CONTROL_BYTE_PERMITS {
            return Err(RuntimeConfigError::ControlQueueCapacityTooLarge {
                capacity_bytes: max_frame_bytes,
                maximum: MAX_CONTROL_BYTE_PERMITS,
            });
        }

        let data_queue_capacity_bytes = data_queue_capacity(
            limits.max_concurrent_transfers,
            limits.max_transfer_bytes_inflight,
            max_data_payload_bytes,
        )?;
        let (upload_ingress_capacity_bytes, upload_ingress_capacity_messages) =
            upload_ingress_capacity(limits.max_concurrent_transfers, max_data_payload_bytes)?;
        let control_queue_capacity_messages = writer_queue_capacity(
            "control",
            limits.max_inflight_requests,
            CONTROL_QUEUE_ALLOWANCE,
            1,
        )?;
        let data_queue_capacity_messages =
            writer_queue_capacity("DATA", limits.max_concurrent_transfers, 0, 1)?;

        if limits.transfer_floor_bits_per_second == 0 {
            return Err(RuntimeConfigError::ZeroTransferFloor);
        }

        let checked = Self {
            max_frame_bytes,
            max_data_payload_bytes,
            data_queue_capacity_bytes,
            upload_ingress_capacity_bytes,
            upload_ingress_capacity_messages,
            control_queue_capacity_messages,
            data_queue_capacity_messages,
            max_concurrent_transfers: limits.max_concurrent_transfers,
            max_transfer_bytes_inflight: limits.max_transfer_bytes_inflight,
            transfer_connect_allowance_ms: limits.transfer_connect_allowance_ms,
            transfer_floor_bits_per_second: limits.transfer_floor_bits_per_second,
            transfer_stall_ms: limits.transfer_stall_ms,
            transfer_stall: Duration::from_millis(limits.transfer_stall_ms),
        };

        // One transfer can consume the entire aggregate byte reservation. If
        // its deadline cannot be represented, the served configuration is not
        // executable and must fail before readiness rather than at admission.
        let now = Instant::now();
        checked.stall_deadline(now)?;
        checked.deadline(now, limits.max_transfer_bytes_inflight)?;
        Ok(checked)
    }

    /// Maximum payload bytes in one complete Text or Binary data message.
    pub(crate) const fn max_frame_bytes(self) -> usize {
        self.max_frame_bytes
    }

    /// Effective `min(65_536, max_frame_bytes - 48)` DATA payload bound.
    pub(crate) const fn max_data_payload_bytes(self) -> usize {
        self.max_data_payload_bytes
    }

    /// Byte permits reserved for at most one queued DATA record per transfer.
    pub(crate) const fn data_queue_capacity_bytes(self) -> usize {
        self.data_queue_capacity_bytes
    }

    /// Complete inbound DATA bytes retained across every upload on one
    /// connection. This covers the largest legal credit window even when it
    /// is split into one-byte records, so a producer that obeys CREDIT cannot
    /// block the socket reader before a following control message is read.
    pub(crate) const fn upload_ingress_capacity_bytes(self) -> usize {
        self.upload_ingress_capacity_bytes
    }

    /// Per-upload DATA record slots for the largest legal credit window.
    pub(crate) const fn upload_ingress_capacity_messages(self) -> usize {
        self.upload_ingress_capacity_messages
    }

    /// Aggregate queued Text/control bytes. One maximum-size message always
    /// fits, while concurrent producers wait for writer acknowledgement.
    pub(crate) const fn control_queue_capacity_bytes(self) -> usize {
        self.max_frame_bytes
    }

    /// Bounded priority-writer queue slots, including control headroom.
    pub(crate) const fn control_queue_capacity_messages(self) -> usize {
        self.control_queue_capacity_messages
    }

    /// Bounded DATA-writer queue slots (one per configured transfer, floored at one).
    pub(crate) const fn data_queue_capacity_messages(self) -> usize {
        self.data_queue_capacity_messages
    }

    /// Zero-forward-progress interval.
    pub(crate) const fn transfer_stall(self) -> Duration {
        self.transfer_stall
    }

    /// Checked forward-progress deadline for the supplied progress instant.
    pub(crate) fn stall_deadline(self, start: Instant) -> Result<Instant, RuntimeConfigError> {
        start
            .checked_add(self.transfer_stall)
            .ok_or(RuntimeConfigError::StallDeadlineNotFinite {
                stall_ms: self.transfer_stall_ms,
            })
    }

    /// Absolute deadline and the exact budget reported in a typed error.
    pub(crate) fn deadline(
        self,
        start: Instant,
        total_bytes: u64,
    ) -> Result<(Instant, u64), RuntimeConfigError> {
        let budget_ms = deadline_budget_ms(
            self.transfer_connect_allowance_ms,
            self.transfer_floor_bits_per_second,
            total_bytes,
        )?;
        let deadline = start
            .checked_add(Duration::from_millis(budget_ms))
            .ok_or(RuntimeConfigError::DeadlineNotFinite { budget_ms })?;
        Ok((deadline, budget_ms))
    }
}

/// A daemon-global admission pool shared by every cloned [`crate::AppState`].
#[derive(Debug, Clone)]
pub(crate) struct TransferPool {
    inner: Arc<TransferPoolInner>,
}

#[derive(Debug)]
struct TransferPoolInner {
    max_concurrent_transfers: u64,
    max_transfer_bytes_inflight: u64,
    state: Mutex<TransferPoolState>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct TransferPoolState {
    active_transfers: u64,
    reserved_bytes: u64,
}

impl TransferPool {
    /// Construct a pool over the two independently served transfer resources.
    pub(crate) fn new(max_concurrent_transfers: u64, max_transfer_bytes_inflight: u64) -> Self {
        Self {
            inner: Arc::new(TransferPoolInner {
                max_concurrent_transfers,
                max_transfer_bytes_inflight,
                state: Mutex::new(TransferPoolState::default()),
            }),
        }
    }

    /// Construct the daemon-global pool from validated runtime limits.
    pub(crate) fn from_runtime(limits: &RuntimeLimits) -> Self {
        Self::new(
            limits.max_concurrent_transfers,
            limits.max_transfer_bytes_inflight,
        )
    }

    /// Atomically reserve one transfer and its complete logical byte total.
    ///
    /// Count is checked first, then checked byte arithmetic and the byte limit,
    /// matching the deterministic admission order. No state changes on error.
    pub(crate) fn reserve(&self, total_bytes: u64) -> Result<TransferReservation, ApiError> {
        let mut state = lock_unpoisoned(&self.inner.state);
        if state.active_transfers >= self.inner.max_concurrent_transfers {
            return Err(resource_exhausted(
                "max_concurrent_transfers",
                self.inner.max_concurrent_transfers,
            ));
        }

        let Some(next_bytes) = state.reserved_bytes.checked_add(total_bytes) else {
            return Err(resource_exhausted(
                "max_transfer_bytes_inflight",
                self.inner.max_transfer_bytes_inflight,
            ));
        };
        if next_bytes > self.inner.max_transfer_bytes_inflight {
            return Err(resource_exhausted(
                "max_transfer_bytes_inflight",
                self.inner.max_transfer_bytes_inflight,
            ));
        }

        // `active_transfers < max`; therefore the increment cannot overflow
        // even when the configured maximum is u64::MAX.
        state.active_transfers += 1;
        state.reserved_bytes = next_bytes;
        drop(state);

        Ok(TransferReservation {
            inner: Arc::clone(&self.inner),
            total_bytes,
        })
    }

    #[cfg(test)]
    fn state(&self) -> TransferPoolState {
        *lock_unpoisoned(&self.inner.state)
    }

    #[cfg(test)]
    pub(crate) fn usage(&self) -> (u64, u64) {
        let state = self.state();
        (state.active_transfers, state.reserved_bytes)
    }
}

/// One non-cloneable transfer admission; dropping it releases both resources.
#[derive(Debug)]
pub(crate) struct TransferReservation {
    inner: Arc<TransferPoolInner>,
    total_bytes: u64,
}

impl Drop for TransferReservation {
    fn drop(&mut self) {
        let mut state = lock_unpoisoned(&self.inner.state);
        // These subtractions are guaranteed by construction: state is mutated
        // only while holding this mutex, and a reservation cannot be cloned or
        // dropped twice. Avoid a panicking destructor even if a future edit
        // violates that internal invariant.
        debug_assert!(state.active_transfers > 0);
        debug_assert!(state.reserved_bytes >= self.total_bytes);
        state.active_transfers = state.active_transfers.saturating_sub(1);
        state.reserved_bytes = state.reserved_bytes.saturating_sub(self.total_bytes);
    }
}

/// Constant-space generator for unpredictable, never-reused stream ids.
pub(crate) struct StreamIdGenerator {
    key: Option<[u8; 32]>,
    last_sequence: u128,
}

impl StreamIdGenerator {
    /// Start a fresh connection-local id sequence.
    pub(crate) const fn new() -> Self {
        Self {
            key: None,
            last_sequence: 0,
        }
    }

    /// Generate one full identity for an outstanding request.
    ///
    /// A connection-secret keyed Feistel permutation maps a checked 128-bit
    /// counter onto the full 128-bit wire domain. The permutation makes every
    /// counter value structurally unique without retaining an ever-growing set,
    /// while its keyed round function keeps the counter and future ids hidden
    /// from an observer. The sole counter whose permutation is zero is skipped.
    pub(crate) fn next(&mut self, request_id: u64) -> Result<StreamIdentity, StreamIdError> {
        if request_id > jeliya_codec::MAX_REQUEST_ID {
            return Err(StreamIdError::RequestIdOutOfRange { request_id });
        }
        let key = self.connection_key()?;
        loop {
            let sequence = self
                .last_sequence
                .checked_add(1)
                .ok_or(StreamIdError::SequenceExhausted)?;
            self.last_sequence = sequence;

            let stream_id = permute_stream_counter(&key, sequence);
            if stream_id != 0 {
                return StreamIdentity::new(request_id, stream_id)
                    .map_err(StreamIdError::InvalidIdentity);
            }
            // A permutation has exactly one zero preimage. The next distinct
            // counter therefore cannot also map to zero.
        }
    }

    fn connection_key(&mut self) -> Result<[u8; 32], StreamIdError> {
        if let Some(key) = self.key {
            return Ok(key);
        }
        let mut key = [0_u8; 32];
        getrandom::fill(&mut key).map_err(StreamIdError::RandomUnavailable)?;
        self.key = Some(key);
        Ok(key)
    }

    #[cfg(test)]
    const fn with_test_key(key: [u8; 32]) -> Self {
        Self {
            key: Some(key),
            last_sequence: 0,
        }
    }
}

impl Default for StreamIdGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for StreamIdGenerator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamIdGenerator")
            .field("key_initialized", &self.key.is_some())
            .field("last_sequence", &self.last_sequence)
            .finish()
    }
}

fn permute_stream_counter(key: &[u8; 32], counter: u128) -> u128 {
    let counter = counter.to_be_bytes();
    let mut left = u64::from_be_bytes(counter[..8].try_into().expect("fixed left half"));
    let mut right = u64::from_be_bytes(counter[8..].try_into().expect("fixed right half"));
    for round in 0..STREAM_ID_FEISTEL_ROUNDS {
        let next_left = right;
        let next_right = left ^ stream_id_round(key, round, right);
        left = next_left;
        right = next_right;
    }
    (u128::from(left) << 64) | u128::from(right)
}

fn stream_id_round(key: &[u8; 32], round: u8, right: u64) -> u64 {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(STREAM_ID_DOMAIN);
    hasher.update(&[round]);
    hasher.update(&right.to_be_bytes());
    let output = hasher.finalize();
    u64::from_be_bytes(
        output.as_bytes()[..8]
            .try_into()
            .expect("BLAKE3 output has a fixed first word"),
    )
}

/// Invalid served transfer configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeConfigError {
    /// The wire `u64` frame limit cannot be represented by this host.
    MaxFrameBytesNotRepresentable { max_frame_bytes: u64 },
    /// No nonempty DATA record fits.
    FrameCannotCarryData {
        max_frame_bytes: usize,
        minimum_bytes: usize,
    },
    /// The frame bound cannot carry a shortest streaming Text request.
    FrameCannotCarryStreamRequest {
        max_frame_bytes: usize,
        minimum_bytes: usize,
    },
    /// The bounded one-record-per-transfer DATA queue is not representable.
    DataQueueCapacityNotRepresentable,
    /// Tokio's byte semaphore cannot represent the calculated queue bound.
    DataQueueCapacityTooLarge {
        capacity_bytes: usize,
        maximum: usize,
    },
    /// The legal upload credit window cannot be represented on this host.
    UploadIngressCapacityNotRepresentable,
    /// Tokio's byte semaphore cannot represent the legal upload window.
    UploadIngressCapacityTooLarge {
        capacity_bytes: usize,
        maximum: usize,
    },
    /// The advertised complete-message bound cannot back a byte semaphore.
    ControlQueueCapacityTooLarge {
        capacity_bytes: usize,
        maximum: usize,
    },
    /// A served writer message bound cannot be converted or added on this host.
    WriterQueueCapacityNotRepresentable {
        queue: &'static str,
        configured: u64,
    },
    /// Tokio's bounded channel cannot represent a calculated message count.
    WriterQueueCapacityTooLarge {
        queue: &'static str,
        capacity_messages: usize,
        maximum: usize,
    },
    /// Deadline division by zero is an invalid served configuration.
    ZeroTransferFloor,
    /// The forward-progress interval cannot be represented by the host timer.
    StallDeadlineNotFinite { stall_ms: u64 },
    /// The size-aware deadline budget does not fit the served `u64` field.
    DeadlineBudgetOverflow { total_bytes: u64 },
    /// The host timer cannot represent the otherwise valid budget.
    DeadlineNotFinite { budget_ms: u64 },
}

impl fmt::Display for RuntimeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MaxFrameBytesNotRepresentable { max_frame_bytes } => write!(
                f,
                "max_frame_bytes {max_frame_bytes} is not representable on this host"
            ),
            Self::FrameCannotCarryData {
                max_frame_bytes,
                minimum_bytes,
            } => write!(
                f,
                "max_frame_bytes {max_frame_bytes} cannot carry a DATA record (minimum {minimum_bytes})"
            ),
            Self::FrameCannotCarryStreamRequest {
                max_frame_bytes,
                minimum_bytes,
            } => write!(
                f,
                "max_frame_bytes {max_frame_bytes} cannot carry the shortest stream request (minimum {minimum_bytes})"
            ),
            Self::DataQueueCapacityNotRepresentable => {
                f.write_str("the bounded DATA queue capacity is not representable")
            }
            Self::DataQueueCapacityTooLarge {
                capacity_bytes,
                maximum,
            } => write!(
                f,
                "DATA queue capacity {capacity_bytes} exceeds semaphore maximum {maximum}"
            ),
            Self::UploadIngressCapacityNotRepresentable => {
                f.write_str("the bounded upload ingress capacity is not representable")
            }
            Self::UploadIngressCapacityTooLarge {
                capacity_bytes,
                maximum,
            } => write!(
                f,
                "upload ingress capacity {capacity_bytes} exceeds semaphore maximum {maximum}"
            ),
            Self::ControlQueueCapacityTooLarge {
                capacity_bytes,
                maximum,
            } => write!(
                f,
                "control queue capacity {capacity_bytes} exceeds semaphore maximum {maximum}"
            ),
            Self::WriterQueueCapacityNotRepresentable { queue, configured } => write!(
                f,
                "{queue} writer queue capacity derived from {configured} is not representable"
            ),
            Self::WriterQueueCapacityTooLarge {
                queue,
                capacity_messages,
                maximum,
            } => write!(
                f,
                "{queue} writer queue capacity {capacity_messages} exceeds channel maximum {maximum}"
            ),
            Self::ZeroTransferFloor => {
                f.write_str("transfer_floor_bits_per_second must be nonzero")
            }
            Self::StallDeadlineNotFinite { stall_ms } => write!(
                f,
                "transfer stall interval {stall_ms}ms is not representable by the host timer"
            ),
            Self::DeadlineBudgetOverflow { total_bytes } => write!(
                f,
                "deadline budget for {total_bytes} bytes is not representable as u64 milliseconds"
            ),
            Self::DeadlineNotFinite { budget_ms } => write!(
                f,
                "deadline budget {budget_ms}ms is not representable by the host timer"
            ),
        }
    }
}

impl Error for RuntimeConfigError {}

/// Failure to create a fresh connection-local stream id.
#[derive(Debug)]
pub(crate) enum StreamIdError {
    /// The request id is outside the browser-safe JSON range.
    RequestIdOutOfRange { request_id: u64 },
    /// Every representable connection-local counter value has been consumed.
    SequenceExhausted,
    /// The OS CSPRNG was unavailable.
    RandomUnavailable(getrandom::Error),
    /// Defensive conversion failure after the checks above.
    InvalidIdentity(jeliya_codec::StreamCodecError),
}

impl fmt::Display for StreamIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestIdOutOfRange { request_id } => {
                write!(
                    f,
                    "request id {request_id} is outside the browser-safe range"
                )
            }
            Self::SequenceExhausted => f.write_str("connection-local stream id space exhausted"),
            Self::RandomUnavailable(error) => write!(f, "OS CSPRNG unavailable: {error}"),
            Self::InvalidIdentity(error) => write!(f, "invalid generated stream identity: {error}"),
        }
    }
}

impl Error for StreamIdError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RandomUnavailable(error) => Some(error),
            Self::InvalidIdentity(error) => Some(error),
            Self::RequestIdOutOfRange { .. } | Self::SequenceExhausted => None,
        }
    }
}

fn resource_exhausted(resource: &str, limit: u64) -> ApiError {
    ApiError::ResourceExhausted {
        resource: resource.to_owned(),
        limit,
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn minimum_stream_request_bytes() -> usize {
    let read = Envelope::new(
        0,
        None,
        FileRead {
            room_id: RoomId::new(""),
            file_id: FileId::new(""),
        },
    )
    .expect("zero is a valid request id");
    let share = Envelope::new(
        0,
        None,
        FileShare {
            room_id: RoomId::new(""),
            name: String::new(),
            declared_bytes: 0,
            declared_content_type: String::new(),
        },
    )
    .expect("zero is a valid request id");
    let read_bytes = serde_json::to_vec(&read)
        .expect("the shortest file.read envelope serializes")
        .len();
    let share_bytes = serde_json::to_vec(&share)
        .expect("the shortest file.share envelope serializes")
        .len();
    read_bytes.max(share_bytes)
}

fn writer_queue_capacity(
    queue: &'static str,
    configured: u64,
    additional: usize,
    minimum: usize,
) -> Result<usize, RuntimeConfigError> {
    let configured_capacity = usize::try_from(configured).map_err(|_| {
        RuntimeConfigError::WriterQueueCapacityNotRepresentable { queue, configured }
    })?;
    let capacity_messages = configured_capacity
        .checked_add(additional)
        .ok_or(RuntimeConfigError::WriterQueueCapacityNotRepresentable { queue, configured })?;
    let capacity_messages = capacity_messages.max(minimum);
    if capacity_messages > Semaphore::MAX_PERMITS {
        return Err(RuntimeConfigError::WriterQueueCapacityTooLarge {
            queue,
            capacity_messages,
            maximum: Semaphore::MAX_PERMITS,
        });
    }
    Ok(capacity_messages)
}

fn data_queue_capacity(
    max_concurrent_transfers: u64,
    max_transfer_bytes_inflight: u64,
    max_data_payload_bytes: usize,
) -> Result<usize, RuntimeConfigError> {
    // A transfer that can emit DATA reserves at least one logical byte. Zero
    // byte transfers need no DATA queue slot, so do not charge header space for
    // more streams than the byte budget can make nonempty.
    let data_streams = max_concurrent_transfers.min(max_transfer_bytes_inflight);
    let payload_capacity = u128::from(data_streams)
        .checked_mul(max_data_payload_bytes as u128)
        .ok_or(RuntimeConfigError::DataQueueCapacityNotRepresentable)?
        .min(u128::from(max_transfer_bytes_inflight));
    let header_capacity = u128::from(data_streams)
        .checked_mul(STREAM_HEADER_BYTES as u128)
        .ok_or(RuntimeConfigError::DataQueueCapacityNotRepresentable)?;
    let capacity = payload_capacity
        .checked_add(header_capacity)
        .ok_or(RuntimeConfigError::DataQueueCapacityNotRepresentable)?;
    let capacity_bytes = usize::try_from(capacity)
        .map_err(|_| RuntimeConfigError::DataQueueCapacityNotRepresentable)?;
    if capacity_bytes > Semaphore::MAX_PERMITS {
        return Err(RuntimeConfigError::DataQueueCapacityTooLarge {
            capacity_bytes,
            maximum: Semaphore::MAX_PERMITS,
        });
    }
    Ok(capacity_bytes)
}

fn upload_ingress_capacity(
    max_concurrent_transfers: u64,
    max_data_payload_bytes: usize,
) -> Result<(usize, usize), RuntimeConfigError> {
    // CREDIT grants at most one DATA-payload window per upload at a time.
    // Because DATA is nonempty, splitting that window into one-byte records
    // also maximizes its record count and complete-record header overhead.
    // Reserving that full legal window keeps the sole WebSocket reader free to
    // reach a following END, ABORT, ACK, JSON request, Ping, or Close. Records
    // that fail receive-side policy, continuity, or CREDIT validation bypass
    // this DATA lane as ordered terminal work.
    let messages_per_upload = max_data_payload_bytes.max(1);
    let complete_one_byte_record = STREAM_HEADER_BYTES
        .checked_add(1)
        .ok_or(RuntimeConfigError::UploadIngressCapacityNotRepresentable)?;
    let per_upload_bytes = messages_per_upload
        .checked_mul(complete_one_byte_record)
        .ok_or(RuntimeConfigError::UploadIngressCapacityNotRepresentable)?;
    let concurrent = usize::try_from(max_concurrent_transfers)
        .map_err(|_| RuntimeConfigError::UploadIngressCapacityNotRepresentable)?;
    let capacity_bytes = concurrent
        .checked_mul(per_upload_bytes)
        .ok_or(RuntimeConfigError::UploadIngressCapacityNotRepresentable)?;
    if capacity_bytes > Semaphore::MAX_PERMITS {
        return Err(RuntimeConfigError::UploadIngressCapacityTooLarge {
            capacity_bytes,
            maximum: Semaphore::MAX_PERMITS,
        });
    }
    Ok((capacity_bytes, messages_per_upload))
}

fn deadline_budget_ms(
    connect_allowance_ms: u64,
    floor_bits_per_second: u64,
    total_bytes: u64,
) -> Result<u64, RuntimeConfigError> {
    if floor_bits_per_second == 0 {
        return Err(RuntimeConfigError::ZeroTransferFloor);
    }
    let numerator = u128::from(total_bytes)
        .checked_mul(8)
        .and_then(|value| value.checked_mul(1_000))
        .ok_or(RuntimeConfigError::DeadlineBudgetOverflow { total_bytes })?;
    let floor = u128::from(floor_bits_per_second);
    let transfer_ms = numerator
        .checked_add(floor - 1)
        .ok_or(RuntimeConfigError::DeadlineBudgetOverflow { total_bytes })?
        / floor;
    let budget_ms = u128::from(connect_allowance_ms)
        .checked_add(transfer_ms)
        .ok_or(RuntimeConfigError::DeadlineBudgetOverflow { total_bytes })?;
    u64::try_from(budget_ms).map_err(|_| RuntimeConfigError::DeadlineBudgetOverflow { total_bytes })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier};

    use super::*;

    fn served_limits() -> Limits {
        jeliya_core::typed::limits()
    }

    fn assert_resource(error: ApiError, expected_resource: &str, expected_limit: u64) {
        match error {
            ApiError::ResourceExhausted { resource, limit } => {
                assert_eq!(resource, expected_resource);
                assert_eq!(limit, expected_limit);
            }
            other => panic!("expected resource_exhausted, got {other:?}"),
        }
    }

    #[test]
    fn transfer_count_accepts_exact_limit_and_refuses_one_past_first() {
        let pool = TransferPool::new(2, 0);
        let first = pool.reserve(0).expect("first count slot");
        let second = pool.reserve(0).expect("exact count limit");
        assert_eq!(
            pool.state(),
            TransferPoolState {
                active_transfers: 2,
                reserved_bytes: 0,
            }
        );

        assert_resource(
            pool.reserve(u64::MAX).unwrap_err(),
            "max_concurrent_transfers",
            2,
        );
        drop((first, second));
        assert_eq!(pool.state(), TransferPoolState::default());
    }

    #[test]
    fn transfer_bytes_accept_exact_limit_and_refuse_one_past() {
        let pool = TransferPool::new(3, 10);
        let six = pool.reserve(6).expect("six bytes");
        let four = pool.reserve(4).expect("exact byte limit");
        assert_resource(
            pool.reserve(1).unwrap_err(),
            "max_transfer_bytes_inflight",
            10,
        );
        drop(six);
        let one = pool.reserve(1).expect("released bytes are reusable");
        drop((four, one));
        assert_eq!(pool.state(), TransferPoolState::default());
    }

    #[test]
    fn checked_byte_overflow_is_resource_exhaustion_without_mutation() {
        let pool = TransferPool::new(2, u64::MAX);
        let full = pool.reserve(u64::MAX).expect("full u64 reservation");
        assert_resource(
            pool.reserve(1).unwrap_err(),
            "max_transfer_bytes_inflight",
            u64::MAX,
        );
        assert_eq!(
            pool.state(),
            TransferPoolState {
                active_transfers: 1,
                reserved_bytes: u64::MAX,
            }
        );
        drop(full);
        assert_eq!(pool.state(), TransferPoolState::default());
    }

    #[test]
    fn zero_byte_transfer_consumes_only_a_count_slot() {
        let pool = TransferPool::new(1, 0);
        let reservation = pool.reserve(0).expect("zero-byte transfer is admissible");
        assert_eq!(pool.state().active_transfers, 1);
        assert_eq!(pool.state().reserved_bytes, 0);
        assert_resource(pool.reserve(0).unwrap_err(), "max_concurrent_transfers", 1);
        drop(reservation);
        assert!(pool.reserve(0).is_ok());
    }

    #[test]
    fn concurrent_holders_are_counted_atomically_and_drop_releases_all() {
        const WORKERS: usize = 4;
        let pool = TransferPool::new(WORKERS as u64, WORKERS as u64);
        let admitted = Arc::new(Barrier::new(WORKERS + 1));
        let release = Arc::new(Barrier::new(WORKERS + 1));
        let handles: Vec<_> = (0..WORKERS)
            .map(|_| {
                let pool = pool.clone();
                let admitted = Arc::clone(&admitted);
                let release = Arc::clone(&release);
                std::thread::spawn(move || {
                    let reservation = pool.reserve(1).expect("worker admission");
                    admitted.wait();
                    release.wait();
                    drop(reservation);
                })
            })
            .collect();

        admitted.wait();
        assert_eq!(
            pool.state(),
            TransferPoolState {
                active_transfers: WORKERS as u64,
                reserved_bytes: WORKERS as u64,
            }
        );
        assert_resource(
            pool.reserve(0).unwrap_err(),
            "max_concurrent_transfers",
            WORKERS as u64,
        );
        release.wait();
        for handle in handles {
            handle.join().expect("worker exits cleanly");
        }
        assert_eq!(pool.state(), TransferPoolState::default());
    }

    #[test]
    fn runtime_limits_compute_frame_payload_and_bounded_data_queue() {
        let mut limits = served_limits();
        limits.max_frame_bytes = (STREAM_HEADER_BYTES + 65_536) as u64;
        limits.max_concurrent_transfers = 2;
        limits.max_transfer_bytes_inflight = 200_000;
        let runtime = RuntimeLimits::from_served(&limits).expect("valid runtime limits");
        assert_eq!(runtime.max_frame_bytes(), STREAM_HEADER_BYTES + 65_536);
        assert_eq!(runtime.max_data_payload_bytes(), 65_536);
        assert_eq!(
            runtime.data_queue_capacity_bytes(),
            2 * (STREAM_HEADER_BYTES + 65_536)
        );
        assert_eq!(
            runtime.upload_ingress_capacity_bytes(),
            2 * 65_536 * (STREAM_HEADER_BYTES + 1)
        );
        assert_eq!(runtime.upload_ingress_capacity_messages(), 65_536);
        assert_eq!(
            runtime.control_queue_capacity_messages(),
            usize::try_from(limits.max_inflight_requests).unwrap() + CONTROL_QUEUE_ALLOWANCE
        );
        assert_eq!(runtime.data_queue_capacity_messages(), 2);
        assert_eq!(runtime.transfer_stall(), Duration::from_secs(30));

        limits.max_frame_bytes += 1;
        let capped = RuntimeLimits::from_served(&limits).expect("protocol payload cap applies");
        assert_eq!(capped.max_data_payload_bytes(), 65_536);
    }

    #[test]
    fn data_queue_bound_accounts_only_for_possible_nonempty_streams() {
        let mut limits = served_limits();
        limits.max_concurrent_transfers = 8;
        limits.max_transfer_bytes_inflight = 1;
        let runtime = RuntimeLimits::from_served(&limits).expect("one-byte queue config");
        assert_eq!(runtime.data_queue_capacity_bytes(), STREAM_HEADER_BYTES + 1);

        limits.max_transfer_bytes_inflight = 0;
        let zero = RuntimeLimits::from_served(&limits).expect("zero-byte-only config");
        assert_eq!(zero.data_queue_capacity_bytes(), 0);
        assert_eq!(zero.data_queue_capacity_messages(), 8);
    }

    #[test]
    fn writer_and_frame_permit_bounds_refuse_conversion_overflow_and_tokio_panics() {
        let mut limits = served_limits();
        limits.max_inflight_requests = u64::MAX;
        assert!(matches!(
            RuntimeLimits::from_served(&limits),
            Err(RuntimeConfigError::WriterQueueCapacityNotRepresentable {
                queue: "control",
                configured: u64::MAX,
            })
        ));

        limits = served_limits();
        let maximum_control_bytes =
            u64::try_from(MAX_CONTROL_BYTE_PERMITS).expect("control permit maximum fits u64");
        limits.max_frame_bytes = maximum_control_bytes;
        let exact_frame =
            RuntimeLimits::from_served(&limits).expect("exact control permit maximum");
        assert_eq!(
            exact_frame.control_queue_capacity_bytes(),
            MAX_CONTROL_BYTE_PERMITS
        );

        limits.max_frame_bytes = maximum_control_bytes + 1;
        assert!(matches!(
            RuntimeLimits::from_served(&limits),
            Err(RuntimeConfigError::ControlQueueCapacityTooLarge {
                maximum: MAX_CONTROL_BYTE_PERMITS,
                ..
            })
        ));

        limits = served_limits();
        let maximum = u64::try_from(Semaphore::MAX_PERMITS)
            .expect("Tokio's semaphore maximum fits the wire u64");
        let control_at_maximum = maximum
            .checked_sub(u64::try_from(CONTROL_QUEUE_ALLOWANCE).unwrap())
            .expect("Tokio permits more slots than the control allowance");
        limits.max_inflight_requests = control_at_maximum;
        let exact_control = RuntimeLimits::from_served(&limits).expect("exact channel maximum");
        assert_eq!(
            exact_control.control_queue_capacity_messages(),
            Semaphore::MAX_PERMITS
        );

        limits.max_inflight_requests = maximum;
        assert!(matches!(
            RuntimeLimits::from_served(&limits),
            Err(RuntimeConfigError::WriterQueueCapacityTooLarge {
                queue: "control",
                maximum: Semaphore::MAX_PERMITS,
                ..
            })
        ));

        assert_eq!(
            writer_queue_capacity("DATA", maximum, 0, 1).expect("exact channel maximum"),
            Semaphore::MAX_PERMITS
        );

        let one_past_maximum = maximum
            .checked_add(1)
            .expect("Tokio reserves headroom below u64::MAX");
        let one_past_maximum_usize = Semaphore::MAX_PERMITS
            .checked_add(1)
            .expect("Tokio reserves headroom below usize::MAX");
        assert_eq!(
            writer_queue_capacity("DATA", one_past_maximum, 0, 1),
            Err(RuntimeConfigError::WriterQueueCapacityTooLarge {
                queue: "DATA",
                capacity_messages: one_past_maximum_usize,
                maximum: Semaphore::MAX_PERMITS,
            })
        );

        limits = served_limits();
        limits.max_transfer_bytes_inflight = 0;
        let ingress_bytes_per_upload = limits
            .max_frame_bytes
            .checked_sub(STREAM_HEADER_BYTES as u64)
            .unwrap()
            .min(65_536)
            .checked_mul((STREAM_HEADER_BYTES + 1) as u64)
            .unwrap();
        let ingress_transfers_at_maximum =
            u64::try_from(Semaphore::MAX_PERMITS).unwrap() / ingress_bytes_per_upload;
        limits.max_concurrent_transfers = ingress_transfers_at_maximum;
        let exact_ingress = RuntimeLimits::from_served(&limits)
            .expect("largest complete upload ingress bound below semaphore maximum");
        assert_eq!(
            exact_ingress.upload_ingress_capacity_bytes(),
            usize::try_from(ingress_transfers_at_maximum * ingress_bytes_per_upload).unwrap()
        );

        limits.max_concurrent_transfers = ingress_transfers_at_maximum + 1;
        assert!(matches!(
            RuntimeLimits::from_served(&limits),
            Err(RuntimeConfigError::UploadIngressCapacityTooLarge { .. })
        ));

        limits.max_concurrent_transfers = maximum;
        limits.max_transfer_bytes_inflight = 0;
        assert!(matches!(
            RuntimeLimits::from_served(&limits),
            Err(RuntimeConfigError::UploadIngressCapacityNotRepresentable
                | RuntimeConfigError::UploadIngressCapacityTooLarge { .. })
        ));
    }

    #[test]
    fn frame_must_carry_data_and_the_shortest_stream_request() {
        let mut limits = served_limits();
        limits.max_frame_bytes = STREAM_HEADER_BYTES as u64;
        assert!(matches!(
            RuntimeLimits::from_served(&limits),
            Err(RuntimeConfigError::FrameCannotCarryData { .. })
        ));

        let minimum = minimum_stream_request_bytes();
        assert_eq!(minimum, 102, "the spec-reviewed minimal envelope");
        limits.max_frame_bytes = (minimum - 1) as u64;
        assert_eq!(
            RuntimeLimits::from_served(&limits),
            Err(RuntimeConfigError::FrameCannotCarryStreamRequest {
                max_frame_bytes: minimum - 1,
                minimum_bytes: minimum,
            })
        );
        limits.max_frame_bytes = minimum as u64;
        assert!(RuntimeLimits::from_served(&limits).is_ok());
    }

    #[test]
    fn deadline_formula_uses_exact_ceiling_and_absolute_start() {
        let mut limits = served_limits();
        limits.transfer_connect_allowance_ms = 5_000;
        limits.transfer_floor_bits_per_second = 8_192;
        let runtime = RuntimeLimits::from_served(&limits).expect("valid deadlines");
        let start = Instant::now();

        let (one_deadline, one_budget) = runtime.deadline(start, 1).expect("one-byte deadline");
        assert_eq!(one_budget, 5_001);
        assert_eq!(
            one_deadline.duration_since(start),
            Duration::from_millis(5_001)
        );

        let (exact_deadline, exact_budget) = runtime
            .deadline(start, 1_024)
            .expect("exact division deadline");
        assert_eq!(exact_budget, 6_000);
        assert_eq!(
            exact_deadline.duration_since(start),
            Duration::from_millis(6_000)
        );

        assert_eq!(
            runtime
                .stall_deadline(start)
                .expect("served stall interval is finite")
                .duration_since(start),
            Duration::from_millis(limits.transfer_stall_ms)
        );
    }

    #[test]
    fn served_stall_interval_is_checked_against_tokio_instant() {
        let mut limits = served_limits();
        limits.transfer_stall_ms = u64::MAX;
        let representable =
            Instant::now().checked_add(Duration::from_millis(limits.transfer_stall_ms));
        let configured = RuntimeLimits::from_served(&limits);

        if representable.is_some() {
            let runtime = configured.expect("host timer represents every served stall value");
            assert!(runtime.stall_deadline(Instant::now()).is_ok());
        } else {
            assert_eq!(
                configured,
                Err(RuntimeConfigError::StallDeadlineNotFinite { stall_ms: u64::MAX })
            );
        }
    }

    #[test]
    fn zero_floor_and_worst_total_budget_overflow_refuse_configuration() {
        let mut limits = served_limits();
        limits.transfer_floor_bits_per_second = 0;
        assert_eq!(
            RuntimeLimits::from_served(&limits),
            Err(RuntimeConfigError::ZeroTransferFloor)
        );

        limits.transfer_floor_bits_per_second = 1;
        limits.transfer_connect_allowance_ms = 0;
        limits.max_transfer_bytes_inflight = u64::MAX;
        assert!(matches!(
            RuntimeLimits::from_served(&limits),
            Err(RuntimeConfigError::DataQueueCapacityTooLarge { .. })
                | Err(RuntimeConfigError::DataQueueCapacityNotRepresentable)
                | Err(RuntimeConfigError::DeadlineBudgetOverflow { .. })
        ));

        assert_eq!(
            deadline_budget_ms(u64::MAX, 8_192, 1),
            Err(RuntimeConfigError::DeadlineBudgetOverflow { total_bytes: 1 })
        );
    }

    #[test]
    fn generated_stream_ids_are_full_width_nonzero_unique_and_constant_space() {
        let mut generator = StreamIdGenerator::with_test_key([0x42; 32]);
        let mut ids = HashSet::new();
        let mut high_halves = HashSet::new();
        let mut low_halves = HashSet::new();
        for expected_sequence in 1_u128..=256 {
            let identity = generator.next(7).expect("fresh stream id");
            let raw = identity.stream_id().get();
            assert_ne!(raw, 0);
            assert_ne!(raw, expected_sequence, "the counter leaked on the wire");
            assert!(ids.insert(raw), "stream id was reused");
            high_halves.insert(raw >> 64);
            low_halves.insert(raw & u128::from(u64::MAX));
            assert_eq!(identity.request_id().get(), 7);
        }
        assert_eq!(generator.last_sequence, 256);
        assert!(
            high_halves.len() > 250 && low_halves.len() > 250,
            "both halves must vary pseudorandomly"
        );
        assert!(
            std::mem::size_of::<StreamIdGenerator>() <= 64,
            "generator state must remain constant and small"
        );
    }

    #[test]
    fn stream_id_permutation_crosses_u64_and_is_keyed() {
        let mut first = StreamIdGenerator::with_test_key([0x11; 32]);
        let mut second = StreamIdGenerator::with_test_key([0x22; 32]);
        first.last_sequence = u128::from(u64::MAX) - 1;
        second.last_sequence = first.last_sequence;

        let before_boundary = first.next(1).unwrap().stream_id().get();
        let after_boundary = first.next(2).unwrap().stream_id().get();
        let other_key = second.next(1).unwrap().stream_id().get();
        assert_ne!(before_boundary, after_boundary);
        assert_ne!(before_boundary, other_key);
        assert_eq!(first.last_sequence, u128::from(u64::MAX) + 1);
    }

    #[test]
    fn stream_id_generator_checks_request_and_sequence_bounds() {
        let mut generator = StreamIdGenerator::new();
        assert!(matches!(
            generator.next(jeliya_codec::MAX_REQUEST_ID + 1),
            Err(StreamIdError::RequestIdOutOfRange { .. })
        ));
        assert_eq!(generator.last_sequence, 0, "invalid requests consume no id");
        assert!(
            generator.key.is_none(),
            "invalid requests consume no entropy"
        );

        generator.last_sequence = u128::MAX;
        assert!(matches!(
            generator.next(0),
            Err(StreamIdError::SequenceExhausted)
        ));
    }
}
