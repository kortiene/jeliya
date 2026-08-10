//! The streaming-call surface for the duplex byte-stream operations (#167
//! §D11). **Surface only; depth deferred.**
//!
//! `file.share` and `file.read` are duplex byte-stream operations (protocol
//! §Byte-stream framing), not simple request→reply. This issue defines the
//! seam surface so the client is not painted into a corner, but the executor
//! is out of scope here: the daemon side is #233/#242/#243 and the client
//! kernel's stream hooks are #269 (assigned to #168 by spec §K16 but not
//! shipped with it). Their credit / `OPEN` / `END` / `ABORT` semantics are
//! that layer's to implement against the protocol.
//!
//! What #167 fixes now is the *type*: a [`StreamCall`] exposes (a) a cancel
//! path that maps to [`CallError::Cancelled`] with the [`Execution`]
//! classification preserved, and (b) a terminal `Result<O::Output, CallError>`.
//! The mock is not required to drive full byte-stream framing — only to prove
//! that a `call_stream` cancellation yields a delivery-classified `Cancelled`.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::channel::oneshot;
use futures::future::BoxFuture;
use jeliya_api::Operation;

use crate::error::{CallError, Execution};
use crate::handle::{ClientHandle, Dedup};

impl ClientHandle {
    /// Begin one streaming operation. Returns a [`StreamCall`] whose terminal
    /// resolves to the operation's paired `O::Output` (or a classified
    /// [`CallError`]), and which can be cancelled with a preserved
    /// [`Execution`] classification.
    ///
    /// Depth is deferred: today the terminal is driven by the same erased
    /// dispatch as [`call`](ClientHandle::call), so the mock's scripted
    /// programs and the cancel path are exercised end-to-end. #269 replaces the
    /// body with real credit/OPEN/END/ABORT framing without changing this
    /// surface (the hooks were assigned to #168 but did not ship with it).
    pub fn call_stream<O: Operation>(&self, input: O, dedup: Dedup) -> StreamCall<O>
    where
        O::Output: 'static,
    {
        let terminal = self.dispatch_typed::<O>(input, dedup);
        let (cancel_tx, cancel_rx) = oneshot::channel();
        StreamCall {
            terminal: Some(terminal),
            cancel_rx,
            cancel_tx: Some(cancel_tx),
        }
    }
}

/// An in-flight streaming call: a terminal future plus a cancel path.
///
/// Awaiting the value drives the operation to its terminal
/// `Result<O::Output, CallError>`. Cancelling — via [`cancel`](Self::cancel) or
/// a detached [`StreamCancel`] handle — resolves the terminal to
/// [`CallError::Cancelled`] carrying the [`Execution`] the caller supplies, so
/// the delivery classification is preserved rather than guessed.
pub struct StreamCall<O: Operation>
where
    O::Output: 'static,
{
    /// The in-flight dispatch. `None` once cancellation has released it: the
    /// backend-side reply channel is dropped at cancellation time (not at
    /// `StreamCall` drop), so abandoned work is observably abandoned — the
    /// mock's `deliver_next` purges it instead of consuming its scripted step.
    terminal: Option<BoxFuture<'static, Result<O::Output, CallError>>>,
    cancel_rx: oneshot::Receiver<Execution>,
    cancel_tx: Option<oneshot::Sender<Execution>>,
}

impl<O: Operation> StreamCall<O>
where
    O::Output: 'static,
{
    /// Cancel this call, resolving its terminal to
    /// [`CallError::Cancelled`]` { execution }`. Returns `false` if the call
    /// was already cancelled or its cancel path was detached. The `execution`
    /// the caller passes is authoritative: `DefinitelyNot` if the stream never
    /// opened, `Unknown` once bytes may have gone out — the kernel's stream
    /// hooks (#269) will supply the true value from framing state.
    pub fn cancel(&mut self, execution: Execution) -> bool {
        match self.cancel_tx.take() {
            Some(tx) => tx.send(execution).is_ok(),
            None => false,
        }
    }

    /// Detach a [`StreamCancel`] so another task can cancel this call without
    /// holding the [`StreamCall`] itself. Returns `None` if already detached or
    /// cancelled.
    pub fn cancel_handle(&mut self) -> Option<StreamCancel> {
        self.cancel_tx.take().map(|tx| StreamCancel { tx })
    }
}

impl<O: Operation> Future for StreamCall<O> {
    type Output = Result<O::Output, CallError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // `StreamCall` holds only `Unpin` fields, so a safe re-borrow is sound.
        let this = self.get_mut();
        // A cancellation, if signaled, wins over a late terminal.
        match Pin::new(&mut this.cancel_rx).poll(cx) {
            Poll::Ready(Ok(execution)) => {
                // Release the dispatch now: dropping the terminal drops the
                // backend-side reply channel deterministically at cancellation
                // time, so the backend can observe the abandonment.
                this.terminal = None;
                return Poll::Ready(Err(CallError::Cancelled { execution }));
            }
            // The sender was dropped without cancelling: not a cancellation.
            // (`futures::channel::oneshot::Receiver` is re-poll-safe after
            // completion — it keeps returning `Ready(Err(Canceled))`.)
            Poll::Ready(Err(_)) => {}
            Poll::Pending => {}
        }
        match this.terminal.as_mut() {
            Some(terminal) => terminal.as_mut().poll(cx),
            // Already resolved by cancellation; a poll after completion has no
            // result to give — stay pending per `Future`'s contract.
            None => Poll::Pending,
        }
    }
}

/// A detached cancel path for a [`StreamCall`], usable from another task.
pub struct StreamCancel {
    tx: oneshot::Sender<Execution>,
}

impl StreamCancel {
    /// Cancel the associated [`StreamCall`], resolving its terminal to
    /// [`CallError::Cancelled`]` { execution }`. Returns `false` if the call
    /// already completed and no longer observes the cancellation.
    pub fn cancel(self, execution: Execution) -> bool {
        self.tx.send(execution).is_ok()
    }
}
