//! Byte-bounded, priority-aware WebSocket output.
//!
//! JSON and stream-control records use the priority queue. DATA uses a
//! separate queue whose complete record bytes are reserved before the file is
//! read and released only after the socket writer has acknowledged the
//! message. This keeps source read-ahead and queued DATA independently bounded
//! from the logical transfer reservation.

use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc,
};

use futures_util::{Sink, SinkExt};
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::{Bytes, Message};

/// Result of one command at the single socket writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteReceipt {
    /// The complete WebSocket message was accepted by the sink.
    Sent,
    /// A queued DATA record was invalidated by a stream terminal decision.
    Discarded,
    /// The socket writer was already gone or its sink failed.
    Closed,
}

struct WriterCommand {
    message: Message,
    receipt: Option<oneshot::Sender<WriteReceipt>>,
    byte_permit: Arc<OwnedSemaphorePermit>,
    connection_live: Option<Arc<AtomicBool>>,
    stream_live: Option<Arc<AtomicBool>>,
    on_start: Option<Box<dyn FnOnce() -> bool + Send>>,
    on_sent: Option<Box<dyn FnOnce() + Send>>,
    pending_state: Option<Arc<AtomicU8>>,
}

const WRITE_QUEUED: u8 = 0;
const WRITE_STARTED: u8 = 1;
const WRITE_DONE: u8 = 2;
const WRITE_CANCELLED: u8 = 3;

/// The receiving halves owned by the connection's sole socket writer.
pub(crate) struct WriterQueues {
    control_rx: mpsc::Receiver<WriterCommand>,
    data_rx: mpsc::Receiver<WriterCommand>,
}

/// Cloneable output handle shared by request and transfer actors.
#[derive(Clone)]
pub(crate) struct Outbound {
    control_tx: mpsc::Sender<WriterCommand>,
    data_tx: mpsc::Sender<WriterCommand>,
    control_bytes: Arc<Semaphore>,
    data_bytes: Arc<Semaphore>,
    max_message_bytes: usize,
    connection_live: Arc<AtomicBool>,
}

/// Complete-record byte capacity acquired before a source read.
#[derive(Clone)]
pub(crate) struct DataReservation {
    permit: Arc<OwnedSemaphorePermit>,
    bytes: usize,
}

/// A queued message whose result is acknowledged by the sole socket writer.
pub(crate) struct PendingWrite {
    receipt: oneshot::Receiver<WriteReceipt>,
    state: Arc<AtomicU8>,
}

/// Cloneable cancellation side for a queued DATA write.
#[derive(Clone)]
pub(crate) struct PendingWriteCancel {
    state: Arc<AtomicU8>,
}

fn message_payload_len(message: &Message) -> usize {
    match message {
        Message::Text(text) => text.len(),
        Message::Binary(bytes) | Message::Ping(bytes) | Message::Pong(bytes) => bytes.len(),
        Message::Close(Some(frame)) => frame.reason.len().saturating_add(2),
        Message::Close(None) | Message::Frame(_) => 0,
    }
}

impl PendingWrite {
    pub(crate) async fn wait(self) -> WriteReceipt {
        self.receipt.await.unwrap_or(WriteReceipt::Closed)
    }

    /// Cancels a DATA command only while it is still queued. Once the sole
    /// writer has claimed the command, its bounded sink receipt must be
    /// reconciled because the message may already be peer-visible.
    pub(crate) fn cancellation(&self) -> PendingWriteCancel {
        PendingWriteCancel {
            state: self.state.clone(),
        }
    }
}

impl PendingWriteCancel {
    pub(crate) fn cancel_before_start(&self) -> bool {
        self.state
            .compare_exchange(
                WRITE_QUEUED,
                WRITE_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

impl Outbound {
    /// Constructs independently bounded control and DATA queues.
    pub(crate) fn new(
        control_messages: usize,
        data_messages: usize,
        control_bytes: usize,
        data_bytes: usize,
        max_message_bytes: usize,
    ) -> (Self, WriterQueues) {
        let (control_tx, control_rx) = mpsc::channel(control_messages.max(1));
        let (data_tx, data_rx) = mpsc::channel(data_messages.max(1));
        (
            Self {
                control_tx,
                data_tx,
                control_bytes: Arc::new(Semaphore::new(control_bytes)),
                data_bytes: Arc::new(Semaphore::new(data_bytes)),
                max_message_bytes,
                connection_live: Arc::new(AtomicBool::new(true)),
            },
            WriterQueues {
                control_rx,
                data_rx,
            },
        )
    }

    /// Enqueues one JSON Text message and returns a socket-write receipt.
    pub(crate) async fn text(&self, bytes: Vec<u8>) -> WriteReceipt {
        let text = String::from_utf8(bytes).expect("serialized JSON is UTF-8");
        self.control_wait(Message::Text(text.into())).await
    }

    /// Enqueues Text and runs `on_sent` after the sink accepts the message but
    /// before any waiter is notified. This is the peer-visible retirement
    /// boundary for request identifiers and completed stream bindings.
    pub(crate) async fn text_with_on_sent<F>(&self, bytes: Vec<u8>, on_sent: F) -> WriteReceipt
    where
        F: FnOnce() + Send + 'static,
    {
        let text = String::from_utf8(bytes).expect("serialized JSON is UTF-8");
        self.control_wait_with_hooks(
            Message::Text(text.into()),
            true,
            None,
            None,
            Some(Box::new(on_sent)),
        )
        .await
    }

    /// Enqueues terminal Text with atomic writer-start and successful-send
    /// hooks, mirroring the stream-control sequencing surface.
    pub(crate) async fn text_with_hooks<F, G>(
        &self,
        bytes: Vec<u8>,
        stream_live: Arc<AtomicBool>,
        on_start: F,
        on_sent: G,
    ) -> WriteReceipt
    where
        F: FnOnce() -> bool + Send + 'static,
        G: FnOnce() + Send + 'static,
    {
        let text = String::from_utf8(bytes).expect("serialized JSON is UTF-8");
        self.control_wait_with_hooks(
            Message::Text(text.into()),
            true,
            Some(stream_live),
            Some(Box::new(on_start)),
            Some(Box::new(on_sent)),
        )
        .await
    }

    /// Enqueues a Pong without waiting behind DATA.
    pub(crate) async fn pong(&self, payload: tokio_tungstenite::tungstenite::Bytes) -> bool {
        self.control_nowait(Message::Pong(payload)).await
    }

    /// Enqueues a Close without waiting behind DATA.
    pub(crate) async fn close(
        &self,
        frame: tokio_tungstenite::tungstenite::protocol::CloseFrame,
    ) -> bool {
        let write_live = Arc::new(AtomicBool::new(true));
        tokio::select! {
            biased;
            receipt = self.control_wait_with_hooks(
                Message::Close(Some(frame)),
                false,
                Some(write_live.clone()),
                None,
                None,
            ) => receipt == WriteReceipt::Sent,
            () = tokio::time::sleep(Duration::from_secs(1)) => {
                write_live.store(false, Ordering::Release);
                false
            }
        }
    }

    async fn control_wait(&self, message: Message) -> WriteReceipt {
        self.control_wait_with_hooks(message, true, None, None, None)
            .await
    }

    async fn control_wait_with_hooks(
        &self,
        message: Message,
        connection_bound: bool,
        stream_live: Option<Arc<AtomicBool>>,
        on_start: Option<Box<dyn FnOnce() -> bool + Send>>,
        on_sent: Option<Box<dyn FnOnce() + Send>>,
    ) -> WriteReceipt {
        let Some(byte_permit) = self.reserve_control(message_payload_len(&message)).await else {
            return WriteReceipt::Closed;
        };
        let (receipt_tx, receipt_rx) = oneshot::channel();
        let command = WriterCommand {
            message,
            receipt: Some(receipt_tx),
            byte_permit: Arc::new(byte_permit),
            connection_live: connection_bound.then(|| self.connection_live.clone()),
            stream_live,
            on_start,
            on_sent,
            pending_state: None,
        };
        if self.control_tx.send(command).await.is_err() {
            return WriteReceipt::Closed;
        }
        receipt_rx.await.unwrap_or(WriteReceipt::Closed)
    }

    async fn control_nowait(&self, message: Message) -> bool {
        let Some(byte_permit) = self.reserve_control(message_payload_len(&message)).await else {
            return false;
        };
        self.control_tx
            .send(WriterCommand {
                message,
                receipt: None,
                byte_permit: Arc::new(byte_permit),
                connection_live: Some(self.connection_live.clone()),
                stream_live: None,
                on_start: None,
                on_sent: None,
                pending_state: None,
            })
            .await
            .is_ok()
    }

    async fn reserve_control(&self, bytes: usize) -> Option<OwnedSemaphorePermit> {
        if bytes > self.max_message_bytes {
            return None;
        }
        let permits = u32::try_from(bytes.max(1)).ok()?;
        self.control_bytes
            .clone()
            .acquire_many_owned(permits)
            .await
            .ok()
    }

    /// Acquires complete-record byte capacity without waiting or reading.
    ///
    /// Runtime configuration keeps every individual record and the aggregate
    /// DATA queue within `u32`, the permit unit accepted by Tokio's semaphore.
    pub(crate) fn reserve_data(&self, record_bytes: usize) -> Option<DataReservation> {
        if record_bytes == 0 || record_bytes > self.max_message_bytes {
            return None;
        }
        let permits = u32::try_from(record_bytes).ok()?;
        let permit = self
            .data_bytes
            .clone()
            .try_acquire_many_owned(permits)
            .ok()?;
        Some(DataReservation {
            permit: Arc::new(permit),
            bytes: record_bytes,
        })
    }

    /// Enqueues one already-reserved DATA record and waits for writer
    /// acknowledgement. The permit remains owned by the command through the
    /// sink write (or cancellation discard).
    #[cfg(test)]
    pub(crate) async fn data(
        &self,
        reservation: DataReservation,
        bytes: Vec<u8>,
        stream_live: Arc<AtomicBool>,
    ) -> WriteReceipt {
        if bytes.len() != reservation.bytes {
            return WriteReceipt::Closed;
        }
        let (receipt_tx, receipt_rx) = oneshot::channel();
        let command = WriterCommand {
            message: Message::Binary(bytes.into()),
            receipt: Some(receipt_tx),
            byte_permit: reservation.permit,
            connection_live: Some(self.connection_live.clone()),
            stream_live: Some(stream_live),
            on_start: None,
            on_sent: None,
            pending_state: None,
        };
        if self.data_tx.send(command).await.is_err() {
            return WriteReceipt::Closed;
        }
        receipt_rx.await.unwrap_or(WriteReceipt::Closed)
    }

    /// Queues one reserved DATA record without awaiting the socket writer.
    /// The runtime uses the returned receipt in the same select loop as its
    /// deadline, stall timer, and terminal controls.
    pub(crate) fn queue_data_with_start<F>(
        &self,
        reservation: DataReservation,
        bytes: Bytes,
        stream_live: Arc<AtomicBool>,
        on_start: F,
    ) -> Option<PendingWrite>
    where
        F: FnOnce() -> bool + Send + 'static,
    {
        if bytes.len() != reservation.bytes {
            return None;
        }
        let (receipt_tx, receipt) = oneshot::channel();
        let state = Arc::new(AtomicU8::new(WRITE_QUEUED));
        self.data_tx
            .try_send(WriterCommand {
                message: Message::Binary(bytes),
                receipt: Some(receipt_tx),
                byte_permit: reservation.permit,
                connection_live: Some(self.connection_live.clone()),
                stream_live: Some(stream_live),
                on_start: Some(Box::new(on_start)),
                on_sent: None,
                pending_state: Some(state.clone()),
            })
            .ok()?;
        Some(PendingWrite { receipt, state })
    }

    /// Enqueues stream control whose state transition is committed by the
    /// sole writer immediately before it starts the WebSocket message. A
    /// `false` callback result discards the command, allowing an inbound
    /// terminal or timer that won first to prevent a late OPEN/END.
    pub(crate) async fn binary_control_with_start<F>(
        &self,
        bytes: Vec<u8>,
        stream_live: Arc<AtomicBool>,
        on_start: F,
    ) -> WriteReceipt
    where
        F: FnOnce() -> bool + Send + 'static,
    {
        self.control_wait_with_hooks(
            Message::Binary(bytes.into()),
            true,
            Some(stream_live),
            Some(Box::new(on_start)),
            None,
        )
        .await
    }

    /// Enqueues stream control with atomic writer-start and successful-send
    /// hooks. The latter runs before the receipt becomes observable.
    pub(crate) async fn binary_control_with_hooks<F, G>(
        &self,
        bytes: Vec<u8>,
        stream_live: Arc<AtomicBool>,
        on_start: F,
        on_sent: G,
    ) -> WriteReceipt
    where
        F: FnOnce() -> bool + Send + 'static,
        G: FnOnce() + Send + 'static,
    {
        self.control_wait_with_hooks(
            Message::Binary(bytes.into()),
            true,
            Some(stream_live),
            Some(Box::new(on_start)),
            Some(Box::new(on_sent)),
        )
        .await
    }

    #[cfg(test)]
    pub(crate) fn available_data_bytes(&self) -> usize {
        self.data_bytes.available_permits()
    }

    #[cfg(test)]
    fn available_control_bytes(&self) -> usize {
        self.control_bytes.available_permits()
    }

    /// Prevents every queued or future non-Close message from starting. A
    /// connection-fatal path calls this before it queues the Close itself.
    pub(crate) fn invalidate_connection(&self) {
        self.connection_live.store(false, Ordering::Release);
    }
}

impl WriterQueues {
    /// Runs the connection's sole socket writer. The biased selection is
    /// deliberate: each DATA write is one bounded record, then queued JSON,
    /// terminal stream control, Pong, or Close gets first opportunity.
    pub(crate) async fn run<S, E>(mut self, mut sink: S, write_timeout: Duration) -> Result<(), E>
    where
        S: Sink<Message, Error = E> + Unpin,
    {
        loop {
            let command = tokio::select! {
                biased;
                Some(command) = self.control_rx.recv() => command,
                Some(command) = self.data_rx.recv() => command,
                else => break,
            };

            if let Some(state) = &command.pending_state {
                if state
                    .compare_exchange(
                        WRITE_QUEUED,
                        WRITE_STARTED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_err()
                {
                    state.store(WRITE_DONE, Ordering::Release);
                    if let Some(receipt) = command.receipt {
                        let _ = receipt.send(WriteReceipt::Discarded);
                    }
                    continue;
                }
            }

            let discarded = command
                .connection_live
                .as_ref()
                .is_some_and(|live| !live.load(Ordering::Acquire))
                || command
                    .stream_live
                    .as_ref()
                    .is_some_and(|live| !live.load(Ordering::Acquire));
            if discarded {
                if let Some(state) = &command.pending_state {
                    state.store(WRITE_DONE, Ordering::Release);
                }
                if let Some(receipt) = command.receipt {
                    let _ = receipt.send(WriteReceipt::Discarded);
                }
                // Dropping the command here releases its DATA permit.
                continue;
            }

            let WriterCommand {
                message,
                receipt,
                byte_permit,
                connection_live: _,
                stream_live: _,
                on_start,
                on_sent,
                pending_state,
            } = command;
            if on_start.is_some_and(|commit| !commit()) {
                if let Some(state) = &pending_state {
                    state.store(WRITE_DONE, Ordering::Release);
                }
                if let Some(receipt) = receipt {
                    let _ = receipt.send(WriteReceipt::Discarded);
                }
                continue;
            }
            // This watchdog is owned by the writer itself, independently of
            // the socket reader. Consequently a sink stalled in DATA cannot
            // also strand a Pong, push, Close, or transfer terminal while the
            // reader is awaiting that control receipt.
            let result = tokio::time::timeout(write_timeout, sink.send(message)).await;
            // A permit represents bytes retained through the sink await. The
            // drop is the writer acknowledgement for byte-capacity purposes.
            drop(byte_permit);
            if let Some(state) = &pending_state {
                state.store(WRITE_DONE, Ordering::Release);
            }
            match result {
                Ok(Ok(())) => {
                    if let Some(on_sent) = on_sent {
                        on_sent();
                    }
                    if let Some(receipt) = receipt {
                        let _ = receipt.send(WriteReceipt::Sent);
                    }
                }
                Ok(Err(error)) => {
                    if let Some(receipt) = receipt {
                        let _ = receipt.send(WriteReceipt::Closed);
                    }
                    return Err(error);
                }
                Err(_) => {
                    if let Some(receipt) = receipt {
                        let _ = receipt.send(WriteReceipt::Closed);
                    }
                    // Dropping the sink closes the connection. The owner task
                    // independently notifies the reader through `close_tx`.
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::pin::Pin;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use std::task::{Context, Poll};

    use futures_util::Sink;
    use tokio::sync::Notify;
    use tokio::time::Duration;
    use tokio_tungstenite::tungstenite::Message;

    use super::{Outbound, WriteReceipt};

    #[derive(Clone, Default)]
    struct RecordingSink {
        messages: Arc<Mutex<Vec<Message>>>,
    }

    impl Sink<Message> for RecordingSink {
        type Error = Infallible;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.messages
                .lock()
                .expect("recording sink poisoned")
                .push(item);
            Ok(())
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    #[derive(Clone, Default)]
    struct StalledSink {
        started: Arc<Notify>,
    }

    impl Sink<Message> for StalledSink {
        type Error = Infallible;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(self: Pin<&mut Self>, _item: Message) -> Result<(), Self::Error> {
            self.started.notify_one();
            Ok(())
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Pending
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn data_bytes_remain_reserved_until_writer_acknowledges() {
        let (out, queues) = Outbound::new(2, 1, 64, 64, 64);
        let reservation = out.reserve_data(64).expect("exact capacity");
        assert_eq!(out.available_data_bytes(), 0);
        assert!(out.reserve_data(1).is_none());

        let sink = RecordingSink::default();
        let written = sink.messages.clone();
        let writer = tokio::spawn(queues.run(sink, Duration::from_secs(1)));
        let live = Arc::new(AtomicBool::new(true));
        assert_eq!(
            out.data(reservation, vec![7; 64], live).await,
            WriteReceipt::Sent
        );
        assert_eq!(out.available_data_bytes(), 64);
        assert!(matches!(
            written.lock().expect("recording sink poisoned").as_slice(),
            [Message::Binary(_)]
        ));
        drop(out);
        assert!(writer.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn on_sent_runs_only_after_a_successful_sink_send() {
        let (out, queues) = Outbound::new(2, 1, 64, 64, 64);
        let sink = RecordingSink::default();
        let writer = tokio::spawn(queues.run(sink, Duration::from_secs(1)));
        let sent = Arc::new(AtomicBool::new(false));
        let mark_sent = sent.clone();
        assert_eq!(
            out.text_with_on_sent(vec![b'x'], move || {
                mark_sent.store(true, Ordering::Release);
            })
            .await,
            WriteReceipt::Sent
        );
        assert!(sent.load(Ordering::Acquire));

        let live = Arc::new(AtomicBool::new(false));
        let discarded_hook = Arc::new(AtomicBool::new(false));
        let mark_discarded = discarded_hook.clone();
        assert_eq!(
            out.binary_control_with_hooks(
                vec![0; 1],
                live,
                || true,
                move || mark_discarded.store(true, Ordering::Release),
            )
            .await,
            WriteReceipt::Discarded
        );
        assert!(!discarded_hook.load(Ordering::Acquire));
        drop(out);
        assert!(writer.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn data_reservation_must_exactly_match_the_queued_record() {
        let (out, _queues) = Outbound::new(2, 1, 64, 64, 64);
        let reservation = out.reserve_data(64).expect("exact capacity");
        assert_eq!(
            out.data(reservation, vec![7; 63], Arc::new(AtomicBool::new(true)),)
                .await,
            WriteReceipt::Closed
        );
        assert_eq!(out.available_data_bytes(), 64);
    }

    #[tokio::test]
    async fn writer_watchdog_unblocks_data_and_control_when_sink_flush_stalls() {
        tokio::time::pause();
        let (out, queues) = Outbound::new(2, 1, 64, 64, 64);
        let sink = StalledSink::default();
        let started = sink.started.clone();
        let writer = tokio::spawn(queues.run(sink, Duration::from_millis(10)));

        let reservation = out.reserve_data(64).expect("exact capacity");
        let data_out = out.clone();
        let data = tokio::spawn(async move {
            data_out
                .data(reservation, vec![7; 64], Arc::new(AtomicBool::new(true)))
                .await
        });
        started.notified().await;
        let control_out = out.clone();
        let control_sent = Arc::new(AtomicBool::new(false));
        let control_sent_task = control_sent.clone();
        let control = tokio::spawn(async move {
            control_out
                .text_with_hooks(
                    vec![b'x'; 64],
                    Arc::new(AtomicBool::new(true)),
                    || true,
                    move || control_sent_task.store(true, Ordering::Release),
                )
                .await
        });

        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(data.await.unwrap(), WriteReceipt::Closed);
        assert_eq!(control.await.unwrap(), WriteReceipt::Closed);
        assert!(!control_sent.load(Ordering::Acquire));
        assert!(writer.await.unwrap().is_ok());
        assert_eq!(out.available_data_bytes(), 64);
        assert_eq!(out.available_control_bytes(), 64);
    }

    #[tokio::test]
    async fn control_overtakes_live_queued_data() {
        let (out, queues) = Outbound::new(2, 1, 64, 64, 64);
        let reservation = out.reserve_data(64).unwrap();
        let live = Arc::new(AtomicBool::new(true));

        // Fill both queues before the writer starts. The live DATA remains
        // sendable, so observed order proves the biased control selection.
        let data_out = out.clone();
        let data_live = live.clone();
        let data =
            tokio::spawn(async move { data_out.data(reservation, vec![9; 64], data_live).await });
        tokio::task::yield_now().await;
        let control_out = out.clone();
        let control =
            tokio::spawn(async move { control_out.text(br#"{"id":1,"ok":true}"#.to_vec()).await });
        tokio::task::yield_now().await;

        let sink = RecordingSink::default();
        let written = sink.messages.clone();
        let writer = tokio::spawn(queues.run(sink, Duration::from_secs(1)));
        assert_eq!(control.await.unwrap(), WriteReceipt::Sent);
        assert_eq!(data.await.unwrap(), WriteReceipt::Sent);
        assert!(matches!(
            written.lock().expect("recording sink poisoned").as_slice(),
            [Message::Text(_), Message::Binary(_)]
        ));
        assert_eq!(out.available_data_bytes(), 64);
        drop(out);
        assert!(writer.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn terminal_latch_discards_queued_data_and_control_bytes_are_bounded() {
        let (out, queues) = Outbound::new(2, 1, 64, 64, 64);
        let reservation = out.reserve_data(64).unwrap();
        let live = Arc::new(AtomicBool::new(true));
        let data_out = out.clone();
        let data_live = live.clone();
        let data =
            tokio::spawn(async move { data_out.data(reservation, vec![9; 64], data_live).await });
        tokio::task::yield_now().await;
        live.store(false, Ordering::Release);

        let control_out = out.clone();
        let control = tokio::spawn(async move { control_out.text(vec![b'x'; 64]).await });
        tokio::task::yield_now().await;
        assert_eq!(out.available_control_bytes(), 0);
        assert_eq!(
            out.text(vec![b'x'; 65]).await,
            WriteReceipt::Closed,
            "one-past complete messages are refused before enqueue"
        );

        let sink = RecordingSink::default();
        let written = sink.messages.clone();
        let writer = tokio::spawn(queues.run(sink, Duration::from_secs(1)));
        assert_eq!(control.await.unwrap(), WriteReceipt::Sent);
        assert_eq!(data.await.unwrap(), WriteReceipt::Discarded);
        assert!(matches!(
            written.lock().expect("recording sink poisoned").as_slice(),
            [Message::Text(_)]
        ));
        assert_eq!(out.available_control_bytes(), 64);
        assert_eq!(out.available_data_bytes(), 64);
        drop(out);
        assert!(writer.await.unwrap().is_ok());
    }
}
