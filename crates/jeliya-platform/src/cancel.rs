//! The cancellation model (#174 §D12): a [`CancelToken`] and the drop-is-abort
//! contract for dialog and copy operations.
//!
//! Every capability method that opens a user dialog or runs a bounded copy
//! takes a `&CancelToken`. Firing the token — **or dropping the returned
//! future** — aborts the operation, runs its cleanup (delete a partial staged
//! file, dismiss a dialog where the platform allows), and yields
//! [`crate::CapabilityError::Cancelled`]. This is the mechanism behind the
//! security requirement "cancellation must not become success": an aborted
//! operation has exactly one settled outcome, `Cancelled`, and it is not `Ok`.
//!
//! The token is executor-agnostic and `wasm32`-safe: a shared atomic flag plus
//! parked wakers, no wall clock and no runtime. Dropping the future is handled
//! by `async` itself — a dropped future stops making progress and runs its
//! destructors — so the "drop is abort" half needs no token at all; the token
//! exists for the *explicit* cancel that must race an in-flight operation
//! deterministically.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// A cloneable cancellation handle shared by a caller and an in-flight
/// operation.
///
/// Every clone shares one flag (an `Arc` bump), so cancelling any clone
/// cancels the operation. It is cheap to clone into a future that must own its
/// cancellation signal.
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<CancelState>,
}

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    wakers: Mutex<WakerSlots>,
}

/// Parked [`Cancelled`] waiters, keyed per waiter so a re-poll REPLACES its
/// slot (no duplicates), readiness clears it, and `Drop` unregisters it — the
/// same slot discipline as `jeliya-client`'s mock `PendingCall`. A plain
/// `Vec<Waker>` would grow by one clone per re-poll (a `Cancelled` raced
/// against chunked work is re-polled on every work wake) and would retain a
/// dropped waiter's waker — and its executor task — for the token's lifetime.
#[derive(Default)]
struct WakerSlots {
    slots: HashMap<u64, Waker>,
    next: u64,
}

impl CancelToken {
    /// A fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent: firing an already-cancelled token is a
    /// no-op. Wakes every future currently awaiting [`CancelToken::cancelled`]
    /// so a racing operation observes the cancel promptly and settles as
    /// [`crate::CapabilityError::Cancelled`].
    pub fn cancel(&self) {
        // Set the flag first, then drain the wakers: a waker that re-polls
        // immediately (an inline executor) must see the flag already set.
        let was = self.inner.cancelled.swap(true, Ordering::SeqCst);
        if was {
            return;
        }
        let wakers = {
            let mut guard = self.inner.wakers.lock().expect("cancel token poisoned");
            std::mem::take(&mut guard.slots)
        };
        for (_, waker) in wakers {
            waker.wake();
        }
    }

    /// Whether cancellation has been requested. An operation checks this before
    /// starting work and between bounded chunks of a copy.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// A future that resolves as soon as this token is cancelled. An operation
    /// races it against its real work so an explicit cancel settles the
    /// operation deterministically, without a wall clock.
    pub fn cancelled(&self) -> Cancelled {
        Cancelled {
            inner: self.inner.clone(),
            waiter: None,
        }
    }

    /// How many [`Cancelled`] waiters are currently parked — diagnostics for
    /// the leak/dedupe test, mirroring the mock's `parked_waiters`.
    #[cfg(test)]
    fn parked_waiters(&self) -> usize {
        self.inner
            .wakers
            .lock()
            .expect("cancel token poisoned")
            .slots
            .len()
    }
}

impl std::fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// The future returned by [`CancelToken::cancelled`]. Resolves exactly once,
/// when the token is cancelled.
pub struct Cancelled {
    inner: Arc<CancelState>,
    /// This waiter's slot in `CancelState::wakers`, once parked: a re-poll
    /// replaces the slot instead of appending a duplicate, readiness clears
    /// it, and `Drop` unregisters it so a dropped waiter cannot retain its
    /// executor task for the token's lifetime.
    waiter: Option<u64>,
}

impl Cancelled {
    /// Clear this waiter's slot, if parked. Called on every `Ready` return
    /// and from `Drop`, so a settled or abandoned waiter leaves nothing
    /// behind in the shared state.
    fn unregister(&mut self) {
        if let Some(id) = self.waiter.take() {
            let mut guard = self.inner.wakers.lock().expect("cancel token poisoned");
            guard.slots.remove(&id);
        }
    }
}

impl Future for Cancelled {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.inner.cancelled.load(Ordering::SeqCst) {
            this.unregister();
            return Poll::Ready(());
        }
        // Register (or refresh) this waiter's slot before re-checking the
        // flag, so a cancel that fires between the first load and the
        // registration is not lost.
        {
            let mut guard = this.inner.wakers.lock().expect("cancel token poisoned");
            let id = match this.waiter {
                Some(id) => id,
                None => {
                    let id = guard.next;
                    guard.next += 1;
                    this.waiter = Some(id);
                    id
                }
            };
            guard.slots.insert(id, cx.waker().clone());
        }
        if this.inner.cancelled.load(Ordering::SeqCst) {
            // The racing cancel() may have drained the slots before the
            // insert above: clear our own slot so a waiter that returns
            // Ready leaves nothing registered.
            this.unregister();
            return Poll::Ready(());
        }
        Poll::Pending
    }
}

impl Drop for Cancelled {
    fn drop(&mut self) {
        if let Some(id) = self.waiter.take() {
            // No panic on a poisoned lock in Drop: unregistration is
            // best-effort cleanup, and the poisoning panic is the real
            // failure being unwound.
            if let Ok(mut guard) = self.inner.wakers.lock() {
                guard.slots.remove(&id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_token_is_not_cancelled() {
        assert!(!CancelToken::new().is_cancelled());
    }

    #[test]
    fn cancel_sets_the_flag_and_is_idempotent() {
        let token = CancelToken::new();
        token.cancel();
        assert!(token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn a_clone_shares_the_flag() {
        let token = CancelToken::new();
        let clone = token.clone();
        clone.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn the_cancelled_future_resolves_after_cancel() {
        let token = CancelToken::new();
        token.cancel();
        futures::executor::block_on(token.cancelled());
    }

    #[test]
    fn cancelled_waiters_neither_leak_nor_duplicate() {
        use futures::FutureExt;

        let token = CancelToken::new();
        assert_eq!(token.parked_waiters(), 0);

        // One waiter polled twice parks one slot, not two — a re-poll
        // replaces its slot instead of appending a duplicate.
        let mut a = token.cancelled();
        assert!((&mut a).now_or_never().is_none());
        assert!((&mut a).now_or_never().is_none());
        assert_eq!(token.parked_waiters(), 1);

        // A second waiter gets its own slot.
        let mut b = token.cancelled();
        assert!((&mut b).now_or_never().is_none());
        assert_eq!(token.parked_waiters(), 2);

        // Dropping a pending waiter unregisters it, leaving the other intact.
        drop(a);
        assert_eq!(token.parked_waiters(), 1);

        // Cancellation resolves the survivor and clears every slot.
        token.cancel();
        assert!((&mut b).now_or_never().is_some());
        assert_eq!(token.parked_waiters(), 0);

        // A waiter first polled after cancel settles without parking —
        // covering the drain-race residual (a slot inserted while cancel()
        // drains is cleared by the waiter itself on its Ready return).
        let mut c = token.cancelled();
        assert!((&mut c).now_or_never().is_some());
        assert_eq!(token.parked_waiters(), 0);
    }
}
