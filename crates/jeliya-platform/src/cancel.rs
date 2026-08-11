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
    wakers: Mutex<Vec<Waker>>,
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
            std::mem::take(&mut *guard)
        };
        for waker in wakers {
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
        }
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
}

impl Future for Cancelled {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.inner.cancelled.load(Ordering::SeqCst) {
            return Poll::Ready(());
        }
        // Register the waker before re-checking the flag, so a cancel that
        // fires between the first load and the registration is not lost.
        {
            let mut wakers = self.inner.wakers.lock().expect("cancel token poisoned");
            wakers.push(cx.waker().clone());
        }
        if self.inner.cancelled.load(Ordering::SeqCst) {
            return Poll::Ready(());
        }
        Poll::Pending
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
}
