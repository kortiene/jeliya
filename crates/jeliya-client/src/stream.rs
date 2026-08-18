//! The streaming-call surface for the duplex byte-stream operations (#167
//! §D11), now driven by the kernel's stream lifecycle (#269).
//!
//! `file.share` and `file.read` are duplex byte-stream operations (protocol
//! §Byte-stream framing), not simple request→reply. The **framing** — the
//! `JBS2` byte layout, offset arithmetic, and per-kind field rules — stays owned
//! by `jeliya-codec` and the daemon executor (#233/#242/#243). The **client
//! control plane** — the `OPEN/DATA/CREDIT/END/ABORT` state, credit accounting,
//! and the per-stream deadline/stall timers — is the kernel's stream layer
//! ([`crate::kernel::streaming`], #269, assigned to #168 by spec §K16 but
//! shipped here).
//!
//! The surface below is deliberately unchanged: a [`StreamCall`] exposes (a) a
//! cancel path that maps to [`CallError::Cancelled`] with the [`Execution`]
//! classification preserved, and (b) a terminal `Result<O::Output, CallError>`.
//! Its terminal **is** the request's dispatch future (§S9): the kernel admits
//! the Text request, an OPEN record installs the stream state, the record
//! exchange runs under the kernel's bounds, and the terminal Text reply settles
//! this future. Cancellation drops the dispatch future, which drives the
//! kernel's `Input::Cancel` — for a stream past OPEN that emits a client ABORT
//! so the daemon's transfer reservation is released, never a silent drop.

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
    /// The terminal is driven by the same erased dispatch as
    /// [`call`](ClientHandle::call); with a kernel backend, the dispatch admits
    /// the Text request and the kernel's stream layer (#269) runs the
    /// credit/OPEN/DATA/END/ABORT exchange under its bounds, settling this
    /// terminal on the daemon's final Text reply (or a classified stream
    /// failure). The public surface is unchanged; the framing stays owned by
    /// #233/#242/#243.
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
    /// was already cancelled or its cancel path was detached.
    ///
    /// Cancellation drops the underlying dispatch future, which drives the
    /// kernel's `Input::Cancel`: for a stream past OPEN the kernel emits a client
    /// ABORT so the daemon's transfer reservation is released, and retires the
    /// stream's state (§S9). The `execution` the caller passes classifies the
    /// resolved terminal (`DefinitelyNot` if the stream never opened, `Unknown`
    /// once bytes may have gone out); deriving it from the kernel's framing state
    /// instead — superseding the caller's value — needs a backend seam this issue
    /// deliberately leaves untouched, and is tracked as a follow-up.
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
