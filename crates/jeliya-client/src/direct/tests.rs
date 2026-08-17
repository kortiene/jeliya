//! DirectClient adapter tests (§12) — code-phase smoke tests plus the e2e
//! fault-matrix subset that crosses the client ↔ engine boundary: saturation
//! (mailbox bounds, AC-2), and stop-during-call (drain + teardown ordering,
//! AC-5). The full fault matrix (lag, unclean teardown, WS view-level parity)
//! and real aarch64/Android instrumentation belong to #175/#196.

use std::path::PathBuf;
use std::time::Duration;

use jeliya_api::{RoomList, RoomListOut};

use super::{connect_direct, DirectConfig};
use crate::error::{CallError, Execution};
use crate::event::State;
use crate::handle::Dedup;
use crate::KernelLimits;

/// A unique temp data dir for one test, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "jeliya-direct-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        Self(dir)
    }

    fn path(&self) -> PathBuf {
        self.0.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Poll `state` until `pred` holds or the budget elapses.
async fn await_state(handle: &crate::ClientHandle, pred: impl Fn(State) -> bool) -> State {
    for _ in 0..400 {
        let state = handle.state();
        if pred(state) {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    handle.state()
}

/// The engine opens once, the client reaches `Ready`, a typed `room.list` call
/// round-trips through the in-process router (`route → resolve_call →
/// execute_with → serde_json::to_string`) and back to the seam, and `stop`
/// drains and awaits teardown. This is the end-to-end "no JSON envelope, no
/// socket" path: `execute_with` is the only engine call, and the typed daemon
/// verdict surfaces as `CallError::Wire` exactly as it would on the WebSocket
/// adapters — the round-trip, not the specific verdict, is what proves the
/// adapter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_reaches_ready_and_room_list_round_trips() {
    let dir = TempDir::new("lifecycle");
    let handle = connect_direct(DirectConfig::new(dir.path()));
    assert_eq!(handle.state(), State::Idle, "a fresh handle is Idle");

    handle.start();
    let state = await_state(&handle, |s| matches!(s, State::Ready | State::Failed)).await;
    assert_eq!(state, State::Ready, "the first open reaches Ready");

    // A fresh, subject-less engine answers `room.list` with the typed
    // `subject_absent` verdict; a subject-provisioned one answers `Ok`. Either
    // is a faithful typed round-trip — never a transport/local error, which is
    // what a socket-less adapter would wrongly produce if the pipeline were
    // broken.
    let reply = handle.call::<RoomList>(RoomList {}, Dedup::None).await;
    match reply {
        Ok(RoomListOut { .. }) => {}
        Err(ref err) if err.as_wire().is_some() => {}
        Err(other) => panic!("room.list must round-trip to a typed reply, got {other:?}"),
    }

    handle.stop().await;
    assert_eq!(handle.state(), State::Stopped, "stop drains and settles");
}

/// `stop()` before `start()` must resolve cleanly — the kernel settles to
/// `Stopped` with no engine to tear down. This guards the edge where Android
/// teardown is called before the app ever foregrounds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_while_idle_completes_cleanly() {
    let dir = TempDir::new("idle-stop");
    let handle = connect_direct(DirectConfig::new(dir.path()));
    assert_eq!(handle.state(), State::Idle, "fresh handle is Idle");
    handle.stop().await;
    assert_eq!(
        handle.state(),
        State::Stopped,
        "stop() from Idle must resolve and reach Stopped without hanging"
    );
}

/// `stop()` after a terminal `Failed` state (dir already owned → `GateRefused`)
/// must also resolve — the kernel has nothing to drain; teardown is a no-op.
/// Guards the edge where a second-open attempt is cleaned up unconditionally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_while_failed_completes_cleanly() {
    let dir = TempDir::new("failed-stop");
    // Hold ownership so the second client's first-open is refused.
    let blocker = connect_direct(DirectConfig::new(dir.path()));
    blocker.start();
    await_state(&blocker, |s| matches!(s, State::Ready | State::Failed)).await;

    // Second client → AlreadyOwned → GateRefused → terminal Failed, no retry.
    let loser = connect_direct(DirectConfig::new(dir.path()));
    loser.start();
    assert_eq!(
        await_state(&loser, |s| matches!(s, State::Ready | State::Failed)).await,
        State::Failed,
        "a second open of an owned dir must reach Failed"
    );
    // stop() from Failed must resolve — no engine to tear down.
    loser.stop().await;
    assert_eq!(
        loser.state(),
        State::Stopped,
        "stop() from Failed must reach Stopped without hanging"
    );
    blocker.stop().await;
}

/// N cycles of `connect_direct` / `start` / `stop` over the same directory:
/// the `OwnerToken` must be released inside `stop().await` each time so the
/// next iteration can open the engine cleanly. Any ownership leak would cause
/// the second cycle to land `Failed`. This guards AC "One owner controls a
/// canonical data directory" and "repeated start/stop" (§12).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_start_stop_does_not_leak_ownership() {
    let dir = TempDir::new("restart");
    for cycle in 0..3u32 {
        let handle = connect_direct(DirectConfig::new(dir.path()));
        handle.start();
        assert_eq!(
            await_state(&handle, |s| matches!(s, State::Ready | State::Failed)).await,
            State::Ready,
            "cycle {cycle}: must reach Ready (no ownership leak from prior stop)"
        );
        handle.stop().await;
        assert_eq!(
            handle.state(),
            State::Stopped,
            "cycle {cycle}: stop drains to Stopped"
        );
        // `drop(handle)` closes the driver-message channel, letting the
        // background driver task exit. The OwnerToken was already released
        // inside `stop().await`, so the next iteration can acquire immediately.
        drop(handle);
    }
}

/// Two DirectClients over the same data dir: exactly one acquires ownership and
/// reaches `Ready`; the other is refused (`AlreadyOwned` → `GateRefused` →
/// terminal `Failed`, no retry). After the winner stops, the dir is
/// re-acquirable — a third client over it opens cleanly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_first_open_yields_one_owner() {
    let dir = TempDir::new("concurrent");
    let first = connect_direct(DirectConfig::new(dir.path()));
    let second = connect_direct(DirectConfig::new(dir.path()));

    first.start();
    // The winner must be live before the loser tries to open, so ownership is
    // actually contended (both `start`s race the single registry entry).
    let first_state = await_state(&first, |s| matches!(s, State::Ready | State::Failed)).await;
    second.start();
    let second_state = await_state(&second, |s| matches!(s, State::Ready | State::Failed)).await;

    let states = [first_state, second_state];
    assert_eq!(
        states.iter().filter(|s| **s == State::Ready).count(),
        1,
        "exactly one client owns the dir: {states:?}"
    );
    assert_eq!(
        states.iter().filter(|s| **s == State::Failed).count(),
        1,
        "the second is refused, not silently sharing: {states:?}"
    );

    // Release the winner; the dir frees and a third client re-acquires it.
    let winner = if first_state == State::Ready {
        &first
    } else {
        &second
    };
    winner.stop().await;
    drop(second);
    drop(first);

    let third = connect_direct(DirectConfig::new(dir.path()));
    third.start();
    assert_eq!(
        await_state(&third, |s| matches!(s, State::Ready | State::Failed)).await,
        State::Ready,
        "a released dir is re-acquirable"
    );
    third.stop().await;
}

/// With `queue_depth = 0` and `in_flight = 1`, any call arriving while the
/// engine is still processing the first is refused immediately with
/// `QueueFull { resource: "queue_depth", limit: 0 }` and classified
/// `DefinitelyNot` — it never reached the engine. This is the §12
/// "Saturation" bullet and AC "Mailbox and push lanes are bounded" (§9).
///
/// `ClientHandle::call` dispatches synchronously (the comment in handle.rs
/// §D8 note: "dispatch happens now, not lazily on the first poll"), so both
/// calls are enqueued before the engine has a chance to reply, guaranteeing
/// the second dispatch sees a full in-flight slot and zero queue capacity.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_full_when_mailbox_saturated() {
    let dir = TempDir::new("saturation");
    // queue_depth=0: no queuing at all. in_flight=1: exactly one call to the
    // engine at a time. Any second call arriving while one is in-flight is
    // refused immediately — queue_depth=0 leaves no room to park it.
    let config = DirectConfig {
        limits: KernelLimits {
            queue_depth: 0,
            in_flight: 1,
            ..KernelLimits::default()
        },
        ..DirectConfig::new(dir.path())
    };
    let handle = connect_direct(config);
    handle.start();
    assert_eq!(
        await_state(&handle, |s| matches!(s, State::Ready | State::Failed)).await,
        State::Ready,
        "engine opens cleanly"
    );

    // Both dispatches are synchronous — the in-flight slot is taken before the
    // engine has a chance to reply.
    let first = handle.call::<RoomList>(RoomList {}, Dedup::None);
    let second = handle.call::<RoomList>(RoomList {}, Dedup::None);

    let second_result = second.await;
    assert!(
        matches!(
            second_result,
            Err(CallError::QueueFull {
                resource: "queue_depth",
                limit: 0
            })
        ),
        "second call with in-flight full and queue_depth=0 must be QueueFull: {second_result:?}"
    );
    assert_eq!(
        second_result.as_ref().unwrap_err().execution(),
        Execution::DefinitelyNot,
        "QueueFull is classified DefinitelyNot — the call never reached the engine"
    );

    let _ = first.await;
    handle.stop().await;
}

/// `stop()` must drain every accepted call before completing teardown: the
/// stop future must not resolve until (a) every in-progress call has settled
/// (replied or cancelled) and (b) the engine is fully torn down. This is the
/// §12 "Stop during call" bullet and AC "Stop drains or explicitly cancels
/// every accepted call and awaits teardown" (§5.2, §5.3).
///
/// `ClientHandle::call` dispatches synchronously, so the calls are all
/// enqueued before `stop()` is called — a genuine in-flight-during-stop
/// scenario, not a race that could be won by the engine draining first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_drains_in_flight_calls_before_completing_teardown() {
    let dir = TempDir::new("stop-drain");
    let handle = connect_direct(DirectConfig::new(dir.path()));
    handle.start();
    assert_eq!(
        await_state(&handle, |s| matches!(s, State::Ready | State::Failed)).await,
        State::Ready,
        "engine opens cleanly"
    );

    // Dispatch several calls synchronously — they all enter the kernel's queue
    // before stop() is called, so stop must drain them, not skip them.
    let calls: Vec<_> = (0..4)
        .map(|_| handle.call::<RoomList>(RoomList {}, Dedup::None))
        .collect();

    // stop() drains every accepted call (reply or Cancelled) then awaits
    // engine teardown. Must not hang.
    handle.stop().await;
    assert_eq!(
        handle.state(),
        State::Stopped,
        "stop drains and reaches Stopped"
    );

    // After stop().await, every future must already be settled — awaiting them
    // is instant, and the class must be consistent with the drain ordering.
    for (i, call) in calls.into_iter().enumerate() {
        let result = call.await;
        match &result {
            // Replied before the drain reached it.
            Ok(_) | Err(CallError::Wire(_)) => {}
            // Cancelled by the drain: stop settles every admitted call.
            Err(CallError::Cancelled { .. }) => {}
            Err(other) => {
                panic!("call {i} unexpected error class after stop drain: {other:?}")
            }
        }
    }
}
