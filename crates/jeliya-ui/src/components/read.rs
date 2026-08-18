//! The shared pane-read dispatch discipline (§5.3), factored out of `app.rs`'s
//! `room.list` future so every #180 pane reads the same way.
//!
//! [`ready_call`] performs one **idempotent** read: it subscribes before the
//! first state check (so a transition between them is buffered, not missed),
//! dispatches only in [`State::Ready`], and on a transient `Disconnected`
//! parks on a fresh subscription until the next leave-and-re-enter to `Ready`
//! and retries — never rendering a spurious result before the first answer.
//! Terminal errors are returned once. Mutations do **not** go through this
//! helper (they never auto-retry — D5); a pane dispatches those directly and
//! surfaces the ambiguous-outcome state itself.

use futures::StreamExt;
use jeliya_api::Operation;
use jeliya_client::{CallError, ClientEvent, ClientHandle, Dedup, Execution, State};

/// Await `Ready`, dispatch `req` once, and on a transient disconnect park until
/// the connection recovers and retry. Returns the typed output or the terminal
/// [`CallError`]. `req` must be an idempotent read (every #180 read is).
pub async fn ready_call<O>(handle: &ClientHandle, req: O) -> Result<O::Output, CallError>
where
    O: Operation + Clone + 'static,
{
    let mut ready = handle.subscribe();
    loop {
        // Gate on the CURRENT state, re-checked per control event, so a buffered
        // Ready followed by a buffered Interrupted keeps waiting for the next
        // real Ready rather than firing on the stale event.
        while handle.state() != State::Ready {
            if ready.next().await.is_none() {
                // The stream ended before Ready (stopped/failed) — nothing to read.
                return Err(CallError::Disconnected {
                    execution: Execution::DefinitelyNot,
                });
            }
        }
        match handle.call::<O>(req.clone(), Dedup::None).await {
            Ok(out) => return Ok(out),
            // A pure idempotent read that dropped mid-flight retries after the
            // next recovery; every other error is terminal.
            Err(err @ CallError::Disconnected { .. }) => {
                if !park_until_recovered(handle).await {
                    return Err(err);
                }
            }
            Err(other) => return Err(other),
        }
    }
}

/// Park on a FRESH subscription until the connection leaves `Ready` and returns
/// to it (proving a real recovery from evidence dated after the failure).
/// Returns `false` if the stream ends first (the backend stopped).
async fn park_until_recovered(handle: &ClientHandle) -> bool {
    let mut recovery = handle.subscribe();
    let mut left_ready = handle.state() != State::Ready;
    loop {
        if left_ready && handle.state() == State::Ready {
            return true;
        }
        match recovery.next().await {
            Some(ClientEvent::StateChanged { to, .. }) => {
                if to != State::Ready {
                    left_ready = true;
                } else if left_ready {
                    return true;
                }
            }
            Some(_) => continue,
            None => return false,
        }
    }
}
