//! The agreement/skew logic shared by [`crate::TargetResolver::resolve`] and
//! the supervisor's adopt/attach paths: read the portfile, then require
//! loopback, a supported declared generation, a PID-bound health proof, and a
//! supported served generation — in that order.
//!
//! **Ordering is load-bearing** and pinned by two fault cases that a naive
//! order would confuse:
//!
//! - A portfile that *declares* an incompatible generation is a
//!   `GenerationMismatch` **before** any health probe (fault #6: a v1 portfile
//!   with no live daemon must fail-closed as a generation mismatch, not as a
//!   `Stale` "nothing answered"). Clean-slate: never adopt, never migrate.
//! - A portfile that declares a *supported* generation but has no matching
//!   healthy daemon is `Stale` (fault #7): on a spawn path a fresh daemon heals
//!   it; on a dial it is a transient retry.
//!
//! Eviction of a *proven-owned* incompatible incumbent (fault #13) does not go
//! through here — it re-proves ownership independently via
//! [`prove_owned`], because this function short-circuits on the declared
//! mismatch before it would reach the health step.

use std::path::Path;
use std::time::Duration;

use crate::error::SupervisorError;
use crate::generation::Generation;
use crate::health::{self, HealthReport};
use crate::portfile::{self, Portfile};
use crate::supervisor::Timeouts;
use crate::target::advertised_endpoint_is_loopback;

/// A monotonic deadline `budget` from now, SATURATING instead of panicking on an
/// absurd configuration. `Timeouts` is a public type with no upper bound, so a
/// caller setting `teardown`/`evict`/etc. to `Duration::MAX` (or any value past
/// the `Instant` range) must not make `Instant + Duration` overflow and panic
/// before any work runs. On overflow the deadline clamps to a far-future instant,
/// which every call site already treats as "effectively unbounded" via
/// `saturating_duration_since` / `tokio::time::timeout`. Fully checked: no add
/// here can panic.
pub(crate) fn deadline_from(budget: Duration) -> tokio::time::Instant {
    let now = tokio::time::Instant::now();
    now.checked_add(budget)
        // ~1 year is far past any real budget yet always representable from a
        // fresh monotonic `now`; if even that overflowed, fall back to `now`.
        .or_else(|| now.checked_add(Duration::from_secs(60 * 60 * 24 * 365)))
        .unwrap_or(now)
}

/// A portfile that passed every gate, plus the health response that proved it and
/// a handle to the `daemon.lock` inode the proven daemon holds — captured HERE, as
/// the final step of validation, so the adopted-stop exit proof is bound to the
/// SAME identity the health probe just proved (not one re-opened later, which a
/// cleanup tool could have unlinked/replaced with an unrelated process's lock). The
/// resolve/dial path ignores it (dropped with the `Validated`); the adopt path
/// retains it on the `Sidecar`. `None` when the lock was absent/unopenable or the
/// bounded capture timed out — which `stop_adopted` fails closed on.
pub(crate) struct Validated {
    pub portfile: Portfile,
    #[allow(dead_code)] // Held for callers that want the served generation; the
    // adopt path reads it, the resolve path does not.
    pub health: HealthReport,
    pub adopted_lock: Option<fd_lock::RwLock<std::fs::File>>,
}

impl std::fmt::Debug for Validated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `fd_lock::RwLock` is not `Debug`; report only whether a handle was
        // captured, never its inode.
        f.debug_struct("Validated")
            .field("portfile", &self.portfile)
            .field("health", &self.health)
            .field("adopted_lock", &self.adopted_lock.is_some())
            .finish()
    }
}

/// Run the full gate for `data_dir` against `expected`. See the module note for
/// the ordering rationale.
pub(crate) async fn validate_portfile(
    data_dir: &Path,
    expected: Generation,
    strict_portfile_perms: bool,
    timeouts: &Timeouts,
) -> Result<Validated, SupervisorError> {
    let portfile = portfile::read_portfile_bounded(data_dir, strict_portfile_perms).await?;

    // 0. Data-dir binding: the portfile must record THIS directory. A
    //    `daemon.json` copied/restored from another install keeps the original's
    //    pid/port/token/data_dir, so without this a copy's supervisor would pass
    //    validation against the ORIGINAL live daemon (and could SIGTERM it under
    //    eviction). Checked first, before any dial or signal (spec §4 D2:
    //    identity binds to the portfile's recorded location).
    if let Some(mismatch) = data_dir_mismatch(data_dir, &portfile.data_dir) {
        return Err(mismatch);
    }

    // 1. Loopback: a portfile advertising a non-loopback endpoint is refused
    //    before any dial (spec §6.4 step 2 / §7.1).
    if let Some(field) = non_loopback_endpoint(&portfile) {
        return Err(SupervisorError::NonLoopback { field });
    }

    // 2. Declared generation: the portfile's own `protocol`+`storage_generation`
    //    must match. Checked before health so a declared-incompatible portfile
    //    fails closed even with no live daemon (fault #6).
    if !expected.matches(portfile.declared_generation()) {
        return Err(SupervisorError::GenerationMismatch {
            expected,
            actual: portfile.declared_generation(),
        });
    }

    // 3. Health, PID-bound: the answering process on the advertised port must be
    //    the portfile's PID. Defeats a recycled port (unrelated listener → PID
    //    mismatch) and a recycled PID (no jeliyad on this port → probe fails).
    let Some(report) =
        health::probe_health(portfile.port, timeouts.health_connect, timeouts.health_read).await
    else {
        return Err(SupervisorError::Stale {
            port: portfile.port,
        });
    };
    if !report.proves_pid(portfile.pid) {
        return Err(SupervisorError::Stale {
            port: portfile.port,
        });
    }

    // 4. Served generation, fail-closed: health's generation must also match.
    //    Guards a portfile that lied and a daemon that changed generation under
    //    a reused port (the second line of defense the kernel's generation
    //    fence #168 backs up).
    if !expected.matches(report.advertised_generation()) {
        return Err(SupervisorError::GenerationMismatch {
            expected,
            actual: report.advertised_generation(),
        });
    }

    // 5. Bearer token shape (last, so it only rejects an otherwise-VALID, LIVE
    //    daemon's portfile — the corrupt-token case). jeliyad's token is 32
    //    CSPRNG bytes hex-encoded — 64 lowercase hex chars
    //    (`lifecycle::generate_token`). A corrupted token (empty / wrong length /
    //    non-hex) still parses as a `String`, so without this the resolver hands
    //    back an "apparently valid" target that every WebSocket handshake then
    //    rejects — a failure far from its cause. Refuse it here as unusable.
    if !token_is_well_formed(portfile.token()) {
        return Err(SupervisorError::PortfileUnreadable {
            path: portfile::portfile_path(data_dir),
            why: "auth_token is not a 64-char lowercase-hex bearer token".to_owned(),
        });
    }

    // FINAL step, bound to the just-proven identity: snapshot a handle to the
    // `daemon.lock` inode the daemon currently holds. Capturing it here — while the
    // health proof is fresh — rather than re-opening the path later closes the
    // window in which a cleanup tool could unlink/replace the lock with an
    // unrelated process's inode. BOUNDED and OFF the executor; a miss yields `None`.
    // The dial path discards this; the adopt path retains it for `stop_adopted`.
    let adopted_lock = snapshot_held_lock(data_dir, deadline_from(timeouts.teardown)).await;

    Ok(Validated {
        portfile,
        health: report,
        adopted_lock,
    })
}

/// Whether the portfile's recorded `data_dir` fails to match `resolved` (the
/// directory this supervisor manages, already canonicalized in
/// `Supervisor::resolve`). The two are compared by their CANONICAL SPELLING —
/// directly, WITHOUT re-canonicalizing the recorded path. Both sides are already
/// canonical: the supervisor canonicalizes `resolved`, and the daemon writes its
/// own `fs::canonicalize`d `data_dir` into the portfile at startup, so a byte
/// comparison is the correct like-with-like check.
///
/// Re-`canonicalize`ing the RECORDED path would FOLLOW it through the filesystem
/// as it exists NOW, which is an attack surface: the recorded string is untrusted
/// (a copied/rewritten portfile), so a recorded path later replaced with a
/// symlink pointing at THIS directory would resolve equal even though the daemon
/// the portfile names still serves the ORIGINAL directory inode — letting
/// adoption attach to, and eviction signal, a foreign daemon. A stale or
/// rewritten recorded path simply fails to match and is refused (fail-closed): a
/// portfile pointing at a directory that is not the one in hand is never trusted.
/// Returns the error to raise, or `None` when they match.
pub(crate) fn data_dir_mismatch(resolved: &Path, recorded: &str) -> Option<SupervisorError> {
    if Path::new(recorded) == resolved {
        None
    } else {
        Some(SupervisorError::DataDirMismatch {
            resolved: resolved.to_path_buf(),
        })
    }
}

/// Whether `token` has jeliyad's bearer-token shape: exactly 64 lowercase hex
/// characters (32 CSPRNG bytes hex-encoded). Rejects empty, wrong-length, and
/// non-hex tokens that would authenticate no handshake.
fn token_is_well_formed(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The first advertised endpoint (`ws` then `http`) that is present but not
/// loopback, if any. Absent endpoints are fine — the resolver builds its own
/// `127.0.0.1` dial URL; the check exists to catch a portfile that *advertises*
/// a routable endpoint (fault #19).
fn non_loopback_endpoint(portfile: &Portfile) -> Option<&'static str> {
    // Return only WHICH field failed, never the raw value: a corrupted portfile
    // could carry a token in a swapped `ws`/`http` field, and the `NonLoopback`
    // error is logged/displayed by callers (token-redaction boundary, §7.2).
    for (field, candidate) in [
        ("ws", portfile.ws.as_deref()),
        ("http", portfile.http.as_deref()),
    ] {
        if let Some(value) = candidate {
            if !advertised_endpoint_is_loopback(value) {
                return Some(field);
            }
        }
    }
    None
}

/// Prove — independently of generation — that the process at `portfile.pid` is
/// the live daemon on `portfile.port`. This is the ONLY gate that authorizes a
/// signal to an incumbent the supervisor does not own: eviction (spec §6.7)
/// calls it before any SIGTERM, so a recycled/dead PID or a foreign process
/// (fault #14) is never signalled. Returns `false` on any doubt.
pub(crate) async fn prove_owned(portfile: &Portfile, timeouts: &Timeouts) -> bool {
    match health::probe_health(portfile.port, timeouts.health_connect, timeouts.health_read).await {
        Some(report) => report.proves_pid(portfile.pid),
        None => false,
    }
}

/// Prove — for EVICTION — that the daemon at `portfile.pid`/`port` is live, owned
/// (health PID match), AND actually SERVES a generation this build does not
/// support. Eviction signals a foreign process, so it must confirm the incumbent
/// is incompatible from its HEALTH, not merely from a portfile that *declares* it:
/// a compatible daemon whose portfile was concurrently corrupted (or replaced) to
/// declare an incompatible generation must never be SIGTERMed on the strength of
/// the file alone. Returns `false` on any doubt.
pub(crate) async fn prove_owned_incompatible(
    portfile: &Portfile,
    expected: Generation,
    timeouts: &Timeouts,
) -> bool {
    match health::probe_health(portfile.port, timeouts.health_connect, timeouts.health_read).await {
        Some(report) => {
            report.proves_pid(portfile.pid) && !expected.matches(report.advertised_generation())
        }
        None => false,
    }
}

/// Poll `/api/health` until the daemon at `pid`/`port` is gone (connection
/// refused, or a different/absent PID answers), bounded by `budget`. Returns
/// `true` if it went dark in time. Used to confirm an eviction actually took
/// effect before respawning (spec §6.7 / §6.9).
///
/// Note: a dark listener is **not** proof of completed cleanup — the daemon
/// drops its listener at the *start* of `graceful_shutdown` and only then
/// spends its room-close budget, removing `daemon.json` last. The eviction path
/// respawns into a fresh daemon that heals any leftover portfile, so listener
/// death is the right signal there; a caller that will *reuse the data dir*
/// must instead wait for [`wait_portfile_removed`].
pub(crate) async fn wait_health_dark(
    pid: u32,
    port: u16,
    budget: Duration,
    timeouts: &Timeouts,
) -> bool {
    let deadline = deadline_from(budget);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        // Bound the ENTIRE probe (connect + write + read) by the remaining budget,
        // not each phase independently: clamping connect and read to `remaining`
        // separately let a slow connect FOLLOWED BY a stalled read run for ~2×
        // what was left. The outer `timeout(remaining, …)` caps the whole
        // operation; the per-phase `health_connect`/`health_read` stay as the
        // normal per-attempt bounds inside it.
        let still_up = match tokio::time::timeout(
            remaining,
            health::probe_health(port, timeouts.health_connect, timeouts.health_read),
        )
        .await
        {
            // DISAPPEARANCE polling: the incumbent is still PRESENT while the same
            // PID answers — even `ok: false`, as an incompatible daemon draining
            // after SIGTERM reports (it may still hold the dir lock). Only a probe
            // that no longer reaches THIS pid (a different pid, or no answer) means
            // it is gone. Using `proves_pid` (which also requires `ok`) would call a
            // still-live, unhealthy incumbent "dark" and respawn over it.
            Ok(Some(report)) => report.is_same_process(pid),
            Ok(None) => false,
            // The probe outran the remaining budget: we cannot conclude dark (the
            // daemon may be up but slow) and we are at/past the deadline, so stop.
            Err(_elapsed) => return false,
        };
        if !still_up {
            return true;
        }
        // Recompute after the probe; if the deadline passed, stop before sleeping.
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100).min(remaining)).await;
    }
}

/// Poll until the daemon's `daemon.json` under `data_dir` is gone, bounded by
/// `budget`. The daemon removes its portfile as the **final** step of
/// `graceful_shutdown` — after it has dropped its listener AND finished closing
/// rooms — so portfile absence is the completion signal a caller must see
/// before it may safely reuse or remove the data dir (the P2 the review names:
/// a bare health-dark check reports `Graceful` while the daemon is still writing
/// state and holding its lock). Returns `true` if it was removed in time.
pub(crate) async fn wait_portfile_removed(data_dir: &Path, budget: Duration) -> bool {
    let path = portfile::portfile_path(data_dir);
    // Poll OFF the async executor: `try_exists` is a pathname `stat`, which blocks
    // indefinitely on a stalled NFS/FUSE mount, so an inline poll could hang an
    // executor worker past `budget`. The blocking task polls with its own deadline;
    // the outer `timeout` bounds the CALLER even if a single `stat` wedges.
    let poll = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + budget;
        loop {
            // Check the DEADLINE before accepting absence: accepting a removal
            // observed only past it would report `Graceful` for cleanup that
            // finished outside the budget.
            if std::time::Instant::now() >= deadline {
                return false;
            }
            // `try_exists`, not `Path::exists`: the latter maps a stat ERROR (an
            // unreadable dir mid-shutdown) to `false` — the same as genuine absence
            // — which would report cleanup complete when it may not be. Only a
            // confirmed `Ok(false)` counts as removed; a stat error keeps waiting.
            if matches!(path.try_exists(), Ok(false)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    matches!(tokio::time::timeout(budget, poll).await, Ok(Ok(true)))
}

/// A bounded, off-executor "is the portfile present?" probe. `try_exists` is a
/// pathname `stat` that blocks on a stalled NFS/FUSE mount, so it runs on
/// `spawn_blocking` capped at `deadline`. Only a confirmed `Ok(true)` observed
/// WITHIN the budget counts as present; a timeout, stat error, or genuine absence
/// all return `false` — the fail-closed reading `stop_adopted` wants (a portfile
/// whose presence cannot be confirmed cannot anchor the present→absent transition).
pub(crate) async fn portfile_present(data_dir: &Path, deadline: tokio::time::Instant) -> bool {
    let path = portfile::portfile_path(data_dir);
    let budget = deadline.saturating_duration_since(tokio::time::Instant::now());
    let probe = tokio::task::spawn_blocking(move || {
        // Reject a result observed AFTER the deadline: tokio's `timeout` polls the
        // joined probe BEFORE its timer, so at a zero/exhausted budget a completed
        // `stat` could otherwise be accepted out-of-budget. Gate on the deadline
        // inside the task, after the (possibly slow) `stat`.
        matches!(path.try_exists(), Ok(true)) && tokio::time::Instant::now() < deadline
    });
    matches!(tokio::time::timeout(budget, probe).await, Ok(Ok(true)))
}

/// A bounded, off-executor "is the portfile CONFIRMED absent?" probe — the
/// single-shot counterpart to [`portfile_present`] for the owned `shutdown`
/// present→absent completion check. Only a confirmed `Ok(false)` (`try_exists`
/// says the file is really gone) observed WITHIN the budget is `true`; a stat
/// ERROR, a timeout, or a still-present file all return `false`, so cleanup that
/// cannot be confirmed within `teardown` reports `Forced`, never a premature
/// `Graceful`.
pub(crate) async fn portfile_absent(data_dir: &Path, deadline: tokio::time::Instant) -> bool {
    let path = portfile::portfile_path(data_dir);
    let budget = deadline.saturating_duration_since(tokio::time::Instant::now());
    let probe = tokio::task::spawn_blocking(move || {
        // See `portfile_present`: reject an absence observed after the deadline so
        // `shutdown`/`stop_adopted` cannot report `Graceful` for an out-of-budget
        // exit.
        matches!(path.try_exists(), Ok(false)) && tokio::time::Instant::now() < deadline
    });
    matches!(tokio::time::timeout(budget, probe).await, Ok(Ok(true)))
}

/// The daemon's advisory lock file name, matching `jeliyad`'s `LOCKFILE_NAME`.
pub(crate) const LOCKFILE_NAME: &str = "daemon.lock";

/// Open a handle to `<data_dir>/daemon.lock` bound to its CURRENT inode, for
/// [`wait_lock_handle_released`]. Snapshot this BEFORE the stop RPC so the
/// completion check follows the ORIGINAL inode the daemon holds — a cleanup tool
/// unlinking or replacing the path afterward cannot substitute a different lock.
/// `None` when the file is absent or cannot be opened.
pub(crate) fn open_lock_handle(data_dir: &Path) -> Option<fd_lock::RwLock<std::fs::File>> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(data_dir.join(LOCKFILE_NAME))
        .ok()
        .map(fd_lock::RwLock::new)
}

/// Whether a snapshotted lock handle is CURRENTLY held by someone else — the
/// live daemon. Snapshotting the path only proves we opened *an* inode, not that
/// it is the one the daemon holds: if `daemon.lock` was unlinked and replaced
/// before the snapshot, [`open_lock_handle`] captures the fresh REPLACEMENT inode
/// (which nobody holds) while the daemon still holds the original unlinked inode.
/// Taking that replacement exclusively later would read as "released" and promise
/// a premature `Graceful`. So callers verify BEFORE the stop RPC that the snapshot
/// is already locked: an adopted daemon we are about to stop must be holding
/// `daemon.lock` right now, so a `try_write` that SUCCEEDS proves the handle is
/// the wrong inode — the caller then discards it and fails closed. The transient
/// guard from a successful `try_write` is dropped immediately, so this never
/// leaves the lock held. `None` (no handle) is not held.
pub(crate) fn lock_handle_is_held(handle: Option<&mut fd_lock::RwLock<std::fs::File>>) -> bool {
    match handle {
        // Held ONLY when `try_write` fails with `WouldBlock` (another owner — the
        // daemon — holds the advisory lock). Any OTHER error (`Interrupted`, a
        // transient filesystem error) is NOT proof of the daemon's hold: treating
        // it as held could retain an unrelated replacement inode whose later
        // release would report a premature `Graceful`. Fail closed — only
        // `WouldBlock` counts.
        Some(lock) => matches!(
            lock.try_write(),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
        ),
        None => false,
    }
}

/// Snapshot a HELD `daemon.lock` handle for [`crate::Sidecar::stop_adopted`],
/// BOUNDED and OFF the async executor. Opening the lock file is blocking FS I/O
/// that on a stalled NFS/FUSE mount can hang indefinitely (`open` on a hung mount
/// blocks), so the open — and the [`lock_handle_is_held`] probe — run on
/// `spawn_blocking`, capped at `deadline`. The snapshot therefore counts INSIDE
/// the teardown budget instead of stalling an executor thread BEFORE the deadline
/// even exists. Returns the handle ONLY if the live daemon currently HOLDS the
/// lock; a missing/unopenable lock, a lock we could take exclusively (a
/// replaced/unlinked inode), OR a stall past the deadline all yield `None`, which
/// the completion gate maps to a fail-closed `ShutdownTimedOut`.
pub(crate) async fn snapshot_held_lock(
    data_dir: &Path,
    deadline: tokio::time::Instant,
) -> Option<fd_lock::RwLock<std::fs::File>> {
    let dir = data_dir.to_path_buf();
    let budget = deadline.saturating_duration_since(tokio::time::Instant::now());
    let join = tokio::task::spawn_blocking(move || {
        let mut handle = open_lock_handle(&dir);
        if lock_handle_is_held(handle.as_mut()) {
            handle
        } else {
            None
        }
    });
    match tokio::time::timeout(budget, join).await {
        Ok(Ok(handle)) => handle,
        // A stalled open/probe (hung mount) or a panicked blocking task — fail
        // closed: the exit proof could not be captured within the budget.
        _ => None,
    }
}

/// Poll a PRE-OPENED lock handle until it can be taken exclusively — i.e. the
/// daemon RELEASED it (process exited) — bounded by `budget`. Portfile removal is
/// NOT the daemon's final act: it removes the portfile, THEN flushes logs and only
/// then `process::exit`s (jeliyad `main.rs`), holding `daemon.lock` until exit, so
/// an adopted `Graceful` must confirm the lock is free or a caller reusing the dir
/// could wedge its restart. Polling the SAME handle (a fd on the original inode),
/// not reopening by path, means a cleanup tool unlinking/replacing `daemon.lock`
/// cannot fool the check — the fd still refers to the inode the daemon holds.
///
/// A `None` handle FAILS CLOSED (returns `false`): the pre-RPC snapshot collapses
/// both "the lock was absent" and "opening it failed" to `None`, and an adopted
/// daemon we just stopped is contracted to hold `daemon.lock` until it exits — so
/// a missing snapshot means the exit proof could not be captured, NOT that the
/// process is gone. Treating it as released would let `stop_adopted` promise
/// `Graceful` while the daemon may still be flushing logs under the unlinked lock
/// inode; the honest verdict is "unproven", which the caller maps to
/// `ShutdownTimedOut`.
///
/// Takes the handle BY VALUE and polls OFF the async executor: on an NFS/FUSE
/// filesystem whose lock manager stalls, even a non-blocking `flock(LOCK_NB)`
/// blocks in the syscall (its "non-blocking" is about lock CONTENTION, not a
/// stalled server), so an inline poll could hang the executor past `budget`. The
/// blocking task owns the handle and polls with its own deadline; the outer
/// `timeout` bounds the CALLER even if a single probe wedges mid-syscall (the
/// blocking thread may leak, but `stop_adopted` returns within `budget`).
pub(crate) async fn wait_lock_handle_released(
    handle: Option<fd_lock::RwLock<std::fs::File>>,
    budget: Duration,
) -> bool {
    let Some(mut lock) = handle else {
        return false;
    };
    let poll = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + budget;
        loop {
            // Check the DEADLINE before attempting/accepting the lock, every
            // iteration: a release observed only AFTER the budget expired is an
            // out-of-budget exit and must NOT be reported as released (→ `Graceful`).
            // Accepting `try_write` first would let a late release win — and the
            // outer `timeout` cannot close that race, because tokio's `Timeout`
            // polls the joined future BEFORE its timer, so a completed blocking task
            // (especially at a zero remaining budget) wins even when both are ready.
            if std::time::Instant::now() >= deadline {
                return false;
            }
            // A successful exclusive lock means the daemon released it; the guard
            // drops immediately, so the brief hold does not block the next start.
            if lock.try_write().is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    match tokio::time::timeout(budget, poll).await {
        Ok(Ok(released)) => released,
        // A probe wedged on a stalled lock manager, or the task panicked — fail
        // closed (unproven release → the caller reports `ShutdownTimedOut`).
        _ => false,
    }
}

/// Poll until the process `pid` has EXITED — `kill(pid, 0)` (signal 0, an existence
/// check) returns `ESRCH` — bounded by `budget`. This binds the adopted-stop
/// completion proof to the IDENTITY the health probe validated (the portfile's
/// PID), independent of the `daemon.lock` PATH: a cleanup tool that unlinks/replaces
/// the lock, or an UNRELATED process's lock release, cannot masquerade as this
/// daemon's exit. Fail-closed: a recycled PID (reassigned after this daemon exited)
/// reads as still alive → `false` → the caller reports `ShutdownTimedOut`, never a
/// premature `Graceful`. `kill(_, 0)` is a fast syscall with no FS access, so it
/// polls inline. `false` off Unix or for an invalid PID (the [`crate::process::SignalPid`]
/// guard rejects `0`/out-of-range values that could signal a whole group).
pub(crate) async fn wait_process_exited(pid: u32, budget: Duration) -> bool {
    #[cfg(unix)]
    {
        use nix::errno::Errno;
        use nix::sys::signal::kill;
        use nix::unistd::Pid;
        let Some(valid) = crate::process::SignalPid::new(pid) else {
            return false;
        };
        let target = Pid::from_raw(valid.get());
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            // A zombie still occupies the PID (`kill` → `Ok`), so this reports exit
            // only once the process is fully gone — strictly AFTER it released the
            // lock, so it never promises `Graceful` prematurely.
            if matches!(kill(target, None), Err(Errno::ESRCH)) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, budget);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use crate::error::SupervisorError;
    use crate::generation::Generation;
    use crate::supervisor::Timeouts;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap()
    }

    /// Short health timeouts so tests that hit the "nothing answers" path do not
    /// wait 30 s for the spawn or 500 ms per health connect in CI.
    fn short() -> Timeouts {
        Timeouts {
            spawn: Duration::from_millis(200),
            health_connect: Duration::from_millis(50),
            health_read: Duration::from_millis(50),
            teardown: Duration::from_millis(200),
            evict: Duration::from_millis(200),
        }
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sup-validate-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_portfile(dir: &std::path::Path, json: &str) {
        let path = dir.join("daemon.json");
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }

    // --- non_loopback_endpoint unit tests (private fn, visible inside module) ---

    fn portfile_with_ws(ws: Option<&str>, http: Option<&str>) -> crate::portfile::Portfile {
        let ws_field = ws.map(|v| format!(r#","ws":"{v}""#)).unwrap_or_default();
        let http_field = http
            .map(|v| format!(r#","http":"{v}""#))
            .unwrap_or_default();
        let json = format!(
            r#"{{"pid":1,"port":9,"data_dir":"/d","auth_token":"t"{ws_field}{http_field}}}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn non_loopback_endpoint_catches_non_loopback_ws() {
        let pf = portfile_with_ws(Some("ws://1.2.3.4/ws"), None);
        assert!(non_loopback_endpoint(&pf).is_some());
    }

    #[test]
    fn non_loopback_endpoint_catches_non_loopback_http() {
        let pf = portfile_with_ws(None, Some("http://10.0.0.1/"));
        assert!(non_loopback_endpoint(&pf).is_some());
    }

    #[test]
    fn non_loopback_endpoint_accepts_loopback_ws() {
        let pf = portfile_with_ws(Some("ws://127.0.0.1:9/ws"), None);
        assert!(non_loopback_endpoint(&pf).is_none());
    }

    #[test]
    fn non_loopback_endpoint_returns_none_when_both_fields_absent() {
        let pf = portfile_with_ws(None, None);
        assert!(
            non_loopback_endpoint(&pf).is_none(),
            "no ws/http fields → no loopback violation"
        );
    }

    #[test]
    fn non_loopback_endpoint_lookalike_hostname_is_caught() {
        // "127.0.0.1.evil.example" does not parse as an IP and is not "localhost"
        // → refused exactly as the daemon's host_header_is_loopback logic would.
        let pf = portfile_with_ws(Some("ws://127.0.0.1.evil.example/ws"), None);
        assert!(non_loopback_endpoint(&pf).is_some());
    }

    // --- validate_portfile ordering tests (synthetic portfiles, no live daemon) ---

    /// Fault case 19: NonLoopback is returned BEFORE the generation gate and BEFORE
    /// any health probe — a non-loopback portfile must fail even with no daemon
    /// at all, and even if the generation would match.
    #[test]
    fn fault19_non_loopback_portfile_fails_before_health() {
        let dir = tmp_dir("f19");
        // `data_dir` records THIS dir so the data-dir binding check passes and
        // the loopback gate is what fires (the behavior under test).
        write_portfile(
            &dir,
            &format!(
                r#"{{"pid":1,"port":9,"protocol":2,"storage_generation":2,
                   "data_dir":{dir:?},"auth_token":"t","ws":"ws://evil.example/ws"}}"#
            ),
        );
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::NonLoopback { .. })),
            "expected NonLoopback, got: {result:?}"
        );
    }

    /// Fault case 6 / spec §D3 (ordering invariant): a v1 portfile (missing
    /// storage_generation) must yield GenerationMismatch BEFORE any health probe.
    /// If the error were Stale instead, a clean-slate daemon could be adopted by
    /// mistake on a spawn path that "heals" the Stale with a fresh spawn.
    #[test]
    fn fault6_v1_portfile_is_generation_mismatch_not_stale() {
        let dir = tmp_dir("f6");
        // v1 shape: schema, protocol:1, no storage_generation.
        write_portfile(
            &dir,
            &format!(
                r#"{{"schema":1,"pid":1,"port":9,"protocol":1,
                   "data_dir":{dir:?},"auth_token":"t"}}"#
            ),
        );
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::GenerationMismatch { .. })),
            "v1 portfile must be GenerationMismatch (not Stale); got: {result:?}"
        );
    }

    /// A portfile whose declared generation matches but has no listener → Stale.
    /// This is fault case 7. The key difference from fault 6: the generation gate
    /// passes, so the health probe runs and fails, which is the correct Stale path.
    #[test]
    fn fault7_compatible_portfile_with_no_listener_is_stale() {
        let dir = tmp_dir("f7");
        // Grab a free port and immediately release it; nothing will answer there.
        let free_port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let p = l.local_addr().unwrap().port();
            drop(l);
            p
        };
        write_portfile(
            &dir,
            &format!(
                r#"{{"pid":99999,"port":{free_port},"protocol":2,"storage_generation":2,
                   "data_dir":{dir:?},"auth_token":"t"}}"#
            ),
        );
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::Stale { .. })),
            "compatible portfile with no listener must be Stale; got: {result:?}"
        );
    }

    /// The data-dir-copy attack: a `daemon.json` copied from another install
    /// records the ORIGINAL's data dir (and a live pid/port). Validating it
    /// against a DIFFERENT resolved dir must fail closed with `DataDirMismatch`
    /// BEFORE any health probe or signal — otherwise the copy's supervisor would
    /// attach to (or, under eviction, SIGTERM) the original daemon. The recorded
    /// dir here points at a live loopback listener to prove the refusal does not
    /// depend on the daemon being dead: the binding gate fires first.
    #[test]
    fn a_portfile_recording_a_foreign_data_dir_is_refused_before_health() {
        let resolved = tmp_dir("ddbind-resolved");
        let foreign = tmp_dir("ddbind-foreign");
        // A live listener on the recorded port so a missing-listener path cannot
        // be what fails: the mismatch must short-circuit before the probe.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let live_port = listener.local_addr().unwrap().port();
        // The portfile records `foreign`, not `resolved`, but is written INTO
        // `resolved` (as a copy would be).
        write_portfile(
            &resolved,
            &format!(
                r#"{{"pid":1,"port":{live_port},"protocol":2,"storage_generation":2,
                   "data_dir":{foreign:?},"auth_token":"t"}}"#
            ),
        );
        let result = rt().block_on(validate_portfile(
            &resolved,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        drop(listener);
        std::fs::remove_dir_all(&resolved).ok();
        std::fs::remove_dir_all(&foreign).ok();
        assert!(
            matches!(result, Err(SupervisorError::DataDirMismatch { .. })),
            "a portfile recording a foreign data dir must be DataDirMismatch; got: {result:?}"
        );
    }

    /// Fault case 5 (validate path): missing portfile → PortfileMissing.
    #[test]
    fn fault5_missing_portfile_returns_portfile_missing() {
        let dir = tmp_dir("f5");
        // No daemon.json created.
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::PortfileMissing(_))),
            "missing portfile must be PortfileMissing; got: {result:?}"
        );
    }

    /// A truncated portfile (torn write) → PortfileUnreadable (validate path).
    #[test]
    fn truncated_portfile_is_portfile_unreadable() {
        let dir = tmp_dir("trunc");
        write_portfile(&dir, r#"{"pid": 1, "port":"#);
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::PortfileUnreadable { .. })),
            "truncated portfile must be PortfileUnreadable; got: {result:?}"
        );
    }

    /// A portfile with absent ws/http fields must not raise NonLoopback — the
    /// resolver builds its own 127.0.0.1 URL; these fields are informational only.
    /// The test still yields GenerationMismatch (not NonLoopback). `data_dir`
    /// records this dir so the check under test (loopback-skip) is reached, not
    /// short-circuited by the data-dir binding gate.
    #[test]
    fn portfile_without_ws_or_http_field_skips_loopback_check() {
        let dir = tmp_dir("nows");
        write_portfile(
            &dir,
            &format!(
                r#"{{"pid":1,"port":9,"protocol":1,"storage_generation":1,
                   "data_dir":{dir:?},"auth_token":"t"}}"#
            ),
        );
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            !matches!(result, Err(SupervisorError::NonLoopback { .. })),
            "no ws/http fields must not yield NonLoopback"
        );
        // And specifically NOT DataDirMismatch — the binding gate must accept a
        // portfile that records this very dir.
        assert!(
            !matches!(result, Err(SupervisorError::DataDirMismatch { .. })),
            "a portfile recording this dir must pass the data-dir binding gate"
        );
    }

    /// `wait_portfile_removed` returns true as soon as `daemon.json` is gone —
    /// the completion signal `stop_adopted` waits for before promising a
    /// `Graceful` teardown (a dark listener alone is premature: the daemon
    /// removes the portfile only after closing rooms).
    #[test]
    fn wait_portfile_removed_returns_true_once_the_portfile_is_gone() {
        let dir = tmp_dir("pf-gone");
        write_portfile(
            &dir,
            r#"{"pid":1,"port":9,"data_dir":"/d","auth_token":"t"}"#,
        );
        let path = dir.join("daemon.json");
        assert!(path.exists());
        let dir_for_task = dir.clone();
        let dir_for_wait = dir.clone();
        let result = rt().block_on(async move {
            // Remove the portfile after a short delay, as a graceful daemon
            // does at the END of its shutdown.
            let remover = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                std::fs::remove_file(dir_for_task.join("daemon.json")).unwrap();
            });
            let removed = wait_portfile_removed(&dir_for_wait, Duration::from_secs(2)).await;
            let _ = remover.await;
            removed
        });
        std::fs::remove_dir_all(&dir).ok();
        assert!(result, "must observe the portfile removal within budget");
    }

    /// `portfile_present` confirms presence/absence bounded and OFF the executor
    /// (the pre-RPC snapshot for `stop_adopted`). Only a confirmed present portfile
    /// is `true`; absence is `false`.
    #[test]
    fn portfile_present_confirms_presence_and_absence_bounded() {
        let dir = tmp_dir("pf-present");
        write_portfile(
            &dir,
            r#"{"pid":1,"port":9,"data_dir":"/d","auth_token":"t"}"#,
        );
        let probe = |dir: &std::path::Path| {
            rt().block_on(async {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                portfile_present(dir, deadline).await
            })
        };
        assert!(probe(&dir), "a present portfile is confirmed present");
        std::fs::remove_file(dir.join("daemon.json")).unwrap();
        assert!(!probe(&dir), "an absent portfile is not present");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `portfile_absent` confirms absence bounded and OFF the executor (the owned
    /// `shutdown` present→absent completion probe). Only a confirmed-gone portfile
    /// is `true`; a present one is `false`.
    #[test]
    fn portfile_absent_confirms_absence_bounded() {
        let dir = tmp_dir("pf-absent");
        let probe = |dir: &std::path::Path| {
            rt().block_on(async {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                portfile_absent(dir, deadline).await
            })
        };
        assert!(probe(&dir), "a missing portfile is confirmed absent");
        write_portfile(
            &dir,
            r#"{"pid":1,"port":9,"data_dir":"/d","auth_token":"t"}"#,
        );
        assert!(!probe(&dir), "a present portfile is not absent");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A ZERO/exhausted budget must NOT accept even a genuinely-absent portfile:
    /// the deadline is checked INSIDE the probe (after the `stat`), so a result
    /// observed out-of-budget is rejected — otherwise tokio's `timeout` (which
    /// polls the joined probe before its timer) could accept it and let `shutdown`
    /// report a premature `Graceful`.
    #[test]
    fn portfile_absent_rejects_an_out_of_budget_result() {
        let dir = tmp_dir("pf-absent-zero"); // no portfile → genuinely absent
        let confirmed = rt().block_on(async {
            // The deadline is already NOW: any result is out-of-budget.
            let deadline = tokio::time::Instant::now();
            portfile_absent(&dir, deadline).await
        });
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            !confirmed,
            "a zero budget must not accept even a confirmed-absent portfile"
        );
    }

    /// `wait_portfile_removed` returns false when the portfile is never removed
    /// within the budget — so `stop_adopted` surfaces `ShutdownTimedOut` rather
    /// than a premature `Graceful` when cleanup stalls (the P2's honest verdict).
    #[test]
    fn wait_portfile_removed_times_out_when_the_portfile_persists() {
        let dir = tmp_dir("pf-stays");
        write_portfile(
            &dir,
            r#"{"pid":1,"port":9,"data_dir":"/d","auth_token":"t"}"#,
        );
        let result = rt().block_on(wait_portfile_removed(&dir, Duration::from_millis(200)));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            !result,
            "a portfile that never disappears must time out, not report removed"
        );
    }

    #[test]
    fn token_is_well_formed_accepts_only_64_lowercase_hex() {
        // 64 lowercase hex = 32 CSPRNG bytes hex-encoded (jeliyad's token).
        let good = "0123456789abcdef".repeat(4);
        assert_eq!(good.len(), 64);
        assert!(super::token_is_well_formed(&good));
        // Malformed: empty, too short, too long, uppercase, non-hex.
        assert!(!super::token_is_well_formed(""));
        assert!(!super::token_is_well_formed("deadbeef"));
        assert!(!super::token_is_well_formed(&"a".repeat(63)));
        assert!(!super::token_is_well_formed(&"a".repeat(65)));
        assert!(!super::token_is_well_formed(&"A".repeat(64))); // uppercase
        assert!(!super::token_is_well_formed(&format!(
            "g{}",
            "a".repeat(63)
        ))); // non-hex
    }

    /// A corrupted portfile that placed a secret in `data_dir` (swapped fields)
    /// must not leak it: `DataDirMismatch` records only the resolved dir, never the
    /// child-controlled recorded value. Red-before/green-after: the error used to
    /// carry `recorded`, whose Display reproduced it.
    #[test]
    fn data_dir_mismatch_does_not_leak_the_recorded_value() {
        let err = data_dir_mismatch(std::path::Path::new("/real/dir"), "DATA_DIR_SECRET_MARKER")
            .expect("a mismatching recorded dir must be DataDirMismatch");
        let shown = format!("{err}");
        assert!(
            !shown.contains("DATA_DIR_SECRET_MARKER"),
            "the recorded value must not reach the error: {shown}"
        );
    }

    /// The pre-opened lock handle tracks the ORIGINAL inode: it reports "not
    /// released" while the daemon holds the lock — even after the PATH is unlinked
    /// — and "released" once the daemon frees it. This is the completion proof an
    /// adopted `Graceful` needs beyond portfile removal, hardened so a cleanup tool
    /// unlinking/replacing `daemon.lock` cannot fool it.
    #[test]
    fn wait_lock_handle_released_tracks_the_original_inode() {
        let dir = tmp_dir("lockwait");
        let lock_path = dir.join(LOCKFILE_NAME);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        let mut daemon_lock = fd_lock::RwLock::new(file);
        let guard = daemon_lock.try_write().expect("the daemon holds the lock");

        // `wait_lock_handle_released` takes the handle BY VALUE (it polls off the
        // executor), so open a DISTINCT snapshot handle per phase — all BEFORE any
        // unlink, so every one refers to the original held inode, exactly as
        // `stop_adopted`'s single pre-RPC snapshot does.
        let held = open_lock_handle(&dir).expect("open the lock handle");
        let across_unlink = open_lock_handle(&dir).expect("open the lock handle");
        let after_release = open_lock_handle(&dir).expect("open the lock handle");

        // While HELD, a short budget times out (not released).
        assert!(
            !rt().block_on(wait_lock_handle_released(
                Some(held),
                Duration::from_millis(200)
            )),
            "a held lock must not be reported released"
        );

        // Unlink the PATH: our handle still refers to the original inode the daemon
        // holds, so absence of the path must NOT be read as release.
        std::fs::remove_file(&lock_path).ok();
        assert!(
            !rt().block_on(wait_lock_handle_released(
                Some(across_unlink),
                Duration::from_millis(200)
            )),
            "unlinking daemon.lock must not fool the completion check"
        );

        // Release → observed as released.
        drop(guard);
        let freed = rt().block_on(wait_lock_handle_released(
            Some(after_release),
            Duration::from_secs(2),
        ));
        drop(daemon_lock);
        std::fs::remove_dir_all(&dir).ok();
        assert!(freed, "a freed lock must be observed as released");
    }

    /// A ZERO budget must NOT accept even a FREE lock: the deadline is checked
    /// BEFORE `try_write`, so a release only observable out-of-budget is never
    /// reported as the daemon's exit (a premature `Graceful`). Pins the
    /// deadline-first ordering (the try-write-first order let a late release win,
    /// and tokio's `Timeout` polls the joined future before its timer, so the outer
    /// bound could not close that race).
    #[test]
    fn wait_lock_handle_released_rejects_a_zero_budget_release() {
        let dir = tmp_dir("lockzero");
        let lock_path = dir.join(LOCKFILE_NAME);
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        // Nobody holds it — `try_write` WOULD succeed — but a zero budget is
        // out-of-budget, so the deadline-first check refuses it.
        let free = open_lock_handle(&dir).expect("open the lock handle");
        assert!(
            !rt().block_on(wait_lock_handle_released(
                Some(free),
                Duration::from_millis(0)
            )),
            "a zero budget must not accept even a free lock"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A `None` lock snapshot FAILS CLOSED. If `stop_adopted` could not capture a
    /// handle to `daemon.lock` before the RPC (the file was absent or could not be
    /// opened), the daemon's exit cannot be proven, so completion must NOT be
    /// reported. Red-before: the pre-fix `None` arm returned `true` (treated as
    /// released), which let an adopted stop promise `Graceful` while the daemon may
    /// still hold the unlinked lock inode while flushing logs.
    #[test]
    fn wait_lock_handle_released_fails_closed_without_a_snapshot() {
        assert!(
            !rt().block_on(wait_lock_handle_released(None, Duration::from_millis(50))),
            "a missing lock snapshot is unproven, not released"
        );
    }

    /// `lock_handle_is_held` distinguishes the daemon-held lock from an unrelated
    /// (replaced) inode. `stop_adopted` uses it pre-RPC to reject a snapshot that
    /// is NOT the daemon's lock — a free handle would otherwise later lock
    /// trivially and read as the daemon's exit.
    #[test]
    fn lock_handle_is_held_detects_the_daemon_held_lock() {
        let dir = tmp_dir("lockheld");
        let lock_path = dir.join(LOCKFILE_NAME);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        let mut daemon_lock = fd_lock::RwLock::new(file);

        // Nobody holds it yet: an opened handle is NOT held.
        let mut free = open_lock_handle(&dir).expect("open handle");
        assert!(
            !lock_handle_is_held(Some(&mut free)),
            "a free lock is not held"
        );
        drop(free);

        // The daemon takes the lock exclusively → a fresh snapshot IS held.
        let guard = daemon_lock.try_write().expect("daemon holds the lock");
        let mut snap = open_lock_handle(&dir).expect("open handle");
        assert!(
            lock_handle_is_held(Some(&mut snap)),
            "a daemon-held lock is held"
        );

        // A `None` handle is never held.
        assert!(!lock_handle_is_held(None), "no handle is not held");

        drop(guard);
        drop(snap);
        drop(daemon_lock);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `snapshot_held_lock` captures the daemon-held lock (bounded, off-executor)
    /// and FAILS CLOSED (`None`) when the lock is not held or absent — the same
    /// contract as the pre-round-19 inline open + `lock_handle_is_held`, now moved
    /// off the runtime thread so a stalled mount cannot hang it.
    #[test]
    fn snapshot_held_lock_captures_held_and_fails_closed_otherwise() {
        let dir = tmp_dir("snapheld");
        let lock_path = dir.join(LOCKFILE_NAME);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        let mut daemon_lock = fd_lock::RwLock::new(file);

        let snap = |dir: &std::path::Path| {
            rt().block_on(async {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                snapshot_held_lock(dir, deadline).await
            })
        };

        // Not held yet → None (fail closed).
        assert!(snap(&dir).is_none(), "an unheld lock must not be captured");

        // The daemon holds it exclusively → captured.
        let guard = daemon_lock.try_write().expect("daemon holds the lock");
        assert!(snap(&dir).is_some(), "a daemon-held lock must be captured");
        drop(guard);
        drop(daemon_lock);

        // Absent lock → None (fail closed).
        std::fs::remove_file(&lock_path).ok();
        assert!(snap(&dir).is_none(), "an absent lock must not be captured");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `deadline_from` SATURATES on an absurd budget instead of panicking: a public
    /// `Timeouts` field set to `Duration::MAX` must not overflow `Instant + Duration`
    /// before any work runs. Red-before: the pre-fix `Instant::now() + budget`
    /// panicked on `Duration::MAX`.
    #[test]
    fn deadline_from_saturates_instead_of_panicking() {
        rt().block_on(async {
            let base = tokio::time::Instant::now();
            // Would panic pre-fix; must clamp far into the future instead.
            let far = deadline_from(Duration::MAX);
            assert!(
                far > base + Duration::from_secs(60 * 60 * 24 * 300),
                "an absurd budget saturates far into the future, not a panic"
            );
            // A normal budget is still honored to within scheduling slack.
            let soon = deadline_from(Duration::from_secs(5));
            assert!(
                soon >= base + Duration::from_secs(4) && soon <= base + Duration::from_secs(7),
                "a normal budget lands at ~now + budget"
            );
        });
    }

    /// `data_dir_mismatch` must NOT follow the untrusted recorded path through the
    /// filesystem: a recorded path replaced with a SYMLINK to `resolved` is still a
    /// MISMATCH. Red-before: the pre-fix `canonicalize()` followed the link and
    /// returned `None` (a false match), letting adoption attach to a foreign daemon.
    #[cfg(unix)]
    #[test]
    fn data_dir_mismatch_does_not_follow_a_recorded_symlink() {
        use std::os::unix::fs::symlink;
        let base = tmp_dir("ddsym");
        let resolved = base.join("real");
        let foreign = base.join("foreign");
        std::fs::create_dir_all(&resolved).unwrap();
        // A copied portfile records `foreign`; an attacker later replaces it with a
        // symlink to `resolved`.
        symlink(&resolved, &foreign).unwrap();
        let result = data_dir_mismatch(&resolved, foreign.to_str().unwrap());
        std::fs::remove_dir_all(&base).ok();
        assert!(
            matches!(result, Some(SupervisorError::DataDirMismatch { .. })),
            "a recorded symlink to the resolved dir must NOT be followed; got: {result:?}"
        );
    }

    /// prove_owned returns false when nothing answers on the advertised port.
    /// This is the gate that prevents signalling a recycled/foreign PID (fault #14).
    #[test]
    fn prove_owned_returns_false_when_nothing_answers() {
        let free_port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let p = l.local_addr().unwrap().port();
            drop(l);
            p
        };
        let pf: crate::portfile::Portfile = serde_json::from_str(&format!(
            r#"{{"pid":99999,"port":{free_port},"protocol":2,"storage_generation":2,
               "data_dir":"/d","auth_token":"t"}}"#
        ))
        .unwrap();
        let result = rt().block_on(prove_owned(&pf, &short()));
        assert!(
            !result,
            "prove_owned must be false when nothing answers; a recycled PID must not be signalled"
        );
    }

    /// Finding-B regression: the adopted-daemon exit proof must follow the ADOPTED
    /// inode, not the path. A handle captured at adoption (while the daemon holds
    /// `daemon.lock`) must keep tracking THAT inode even after a cleanup tool
    /// unlinks the path and drops a FREE replacement lock there — a shutdown-time
    /// re-open would instead take the free replacement and falsely read the daemon
    /// as exited. Release is observed only when the ORIGINAL inode is freed.
    #[test]
    fn a_captured_lock_handle_tracks_its_inode_across_a_path_replacement() {
        let rt = rt();

        // Anti-attack: the daemon holds the ORIGINAL inode; a cleanup tool unlinks
        // the path and a DIFFERENT process holds a REPLACEMENT lock there. A re-open
        // of the PATH (the pre-fix shutdown snapshot) captures that replacement, and
        // the unrelated process releasing it falsely reads as the daemon's exit. The
        // handle captured AT ADOPTION follows the original inode — still held by the
        // daemon — so it is NOT fooled.
        let dir = tmp_dir("lock-inode-track");
        let lockpath = dir.join(LOCKFILE_NAME);
        let held_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lockpath)
            .unwrap();
        let mut held = fd_lock::RwLock::new(held_file);
        let guard = held
            .try_write()
            .expect("the daemon holds daemon.lock exclusively");
        // Adoption captures a handle (a separate fd) to the currently-held inode.
        let captured = rt.block_on(snapshot_held_lock(
            &dir,
            deadline_from(Duration::from_secs(2)),
        ));
        assert!(
            captured.is_some(),
            "the held original inode must be captured at adoption"
        );
        // A cleanup tool unlinks daemon.lock; a DIFFERENT process holds a replacement.
        std::fs::remove_file(&lockpath).unwrap();
        let repl_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lockpath)
            .unwrap();
        let mut repl = fd_lock::RwLock::new(repl_file);
        let repl_guard = repl
            .try_write()
            .expect("an unrelated process holds the replacement");
        // The pre-fix behaviour: a shutdown-time re-open captures the HELD
        // REPLACEMENT inode, not the daemon's.
        let reopened = rt.block_on(snapshot_held_lock(
            &dir,
            deadline_from(Duration::from_secs(2)),
        ));
        assert!(
            reopened.is_some(),
            "a shutdown re-open captures the held REPLACEMENT inode (the bug)"
        );
        // The unrelated process releases its lock → the re-opened handle reads as
        // 'released' (the false `Graceful` the fix prevents) …
        drop(repl_guard);
        drop(repl);
        assert!(
            rt.block_on(wait_lock_handle_released(
                reopened,
                Duration::from_millis(300)
            )),
            "the replacement's release fools a re-opened handle"
        );
        // … while the ADOPTION-captured handle follows the original, still held by
        // the daemon, and correctly does NOT read as released.
        assert!(
            !rt.block_on(wait_lock_handle_released(
                captured,
                Duration::from_millis(150)
            )),
            "the retained original-inode handle is not fooled by the replacement's release"
        );
        drop(guard);
        drop(held);
        std::fs::remove_dir_all(&dir).ok();

        // Positive: a captured handle DOES report released once the daemon frees its
        // original lock.
        let dir2 = tmp_dir("lock-inode-release");
        let lockpath2 = dir2.join(LOCKFILE_NAME);
        let held_file2 = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lockpath2)
            .unwrap();
        let mut held2 = fd_lock::RwLock::new(held_file2);
        let guard2 = held2.try_write().expect("held");
        let captured2 = rt.block_on(snapshot_held_lock(
            &dir2,
            deadline_from(Duration::from_secs(2)),
        ));
        assert!(captured2.is_some(), "the held inode must be captured");
        drop(guard2);
        drop(held2); // the daemon exits, releasing its lock
        assert!(
            rt.block_on(wait_lock_handle_released(
                captured2,
                Duration::from_millis(500)
            )),
            "a captured handle must report released once the daemon drops its lock"
        );
        std::fs::remove_dir_all(&dir2).ok();
    }

    /// The identity-bound completion proof: a LIVE PID does not read as exited; a
    /// killed-and-reaped one does. This is what binds the adopted-stop `Graceful`
    /// verdict to the health-proven PID, immune to any `daemon.lock` path swap.
    #[cfg(unix)]
    #[test]
    fn wait_process_exited_tracks_a_process_disappearing() {
        let rt = rt();
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 600"])
            .spawn()
            .expect("spawn a long-lived child");
        let pid = child.id();
        assert!(
            !rt.block_on(wait_process_exited(pid, Duration::from_millis(150))),
            "a live process must not read as exited"
        );
        child.kill().expect("kill the child");
        child.wait().expect("reap the child");
        assert!(
            rt.block_on(wait_process_exited(pid, Duration::from_secs(2))),
            "a killed-and-reaped process must read as exited (ESRCH)"
        );
    }
}
