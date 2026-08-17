//! The supervisor entry point: resolve the binary and data dir, spawn-or-adopt
//! a supervised daemon, validate ready↔portfile↔health agreement, and (opt-in)
//! evict a proven-owned incompatible incumbent.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader, Lines, Take};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::error::SupervisorError;
use crate::generation::Generation;
use crate::portfile::{self, Portfile};
use crate::process;
use crate::sidecar::{Ownership, Sidecar, DEFAULT_TEARDOWN};
use crate::validate;

/// The bounded-time budgets every wait is held to (spec §7.7 — "bounded
/// everything"). All `Copy`; sensible defaults match the daemon's own timings.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    /// How long to wait for the daemon's `ready`/`already_running` line.
    pub spawn: Duration,
    /// TCP connect timeout for a health probe.
    pub health_connect: Duration,
    /// Read timeout for a health probe.
    pub health_read: Duration,
    /// Graceful-teardown budget before escalation (owned) or timeout (adopted).
    pub teardown: Duration,
    /// Budget for a proven-owned incumbent to go dark after an eviction signal.
    pub evict: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            // Matches the spike's 30s announcement budget.
            spawn: Duration::from_secs(30),
            health_connect: Duration::from_millis(500),
            health_read: Duration::from_secs(1),
            teardown: DEFAULT_TEARDOWN,
            evict: DEFAULT_TEARDOWN,
        }
    }
}

/// How to construct a [`Supervisor`].
#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    /// The `{protocol, storage_generation}` this build speaks. Injected, not
    /// compiled in, so mismatches are testable (spec §3.1 / OQ-1).
    pub expected: Generation,
    /// Override the per-user data dir. `None` → the platform default the daemon
    /// itself uses (`dirs::data_dir()/Jeliya`).
    pub data_dir: Option<PathBuf>,
    /// Override binary resolution with an explicit path (equivalent to
    /// `JELIYAD_BIN`). `None` → the fail-closed resolution order (spec §6.1).
    pub binary: Option<PathBuf>,
    /// Pass `--loopback` (the SDK's loopback/CI network mode) to a spawned
    /// daemon.
    pub loopback: bool,
    /// Opt in to replacing a **proven-owned** incompatible incumbent (evict +
    /// respawn). Default off — a pure adopter never evicts (spec §6.7).
    pub replace_incompatible: bool,
    /// Strict portfile-permission mode: on Unix, refuse a group/other-readable
    /// portfile as a token-leak guard. Default off (warn-and-proceed; OQ-3).
    pub strict_portfile_perms: bool,
    /// Bounded-time budgets.
    pub timeouts: Timeouts,
}

impl SupervisorConfig {
    /// A config for `expected`, with platform defaults everywhere else.
    pub fn new(expected: Generation) -> Self {
        Self {
            expected,
            data_dir: None,
            binary: None,
            loopback: false,
            replace_incompatible: false,
            strict_portfile_perms: false,
            timeouts: Timeouts::default(),
        }
    }
}

/// The supervisor: a resolved binary + data dir + expected generation, plus
/// policy. Cheap to hold; the process it manages lives in the [`Sidecar`] a
/// spawn/adopt returns.
#[derive(Debug)]
pub struct Supervisor {
    // Best-effort: `None` when no binary resolved. `start_or_adopt` needs it and
    // fails `NoBinary`; `attach_to_running` does not (spec §6.1).
    binary: Option<PathBuf>,
    binary_tried: Vec<String>,
    data_dir: PathBuf,
    expected: Generation,
    loopback: bool,
    replace_incompatible: bool,
    strict_portfile_perms: bool,
    timeouts: Timeouts,
}

/// The bound for `resolve`'s blocking FS setup (data-dir create/canonicalize and
/// binary-path resolution). A DEDICATED constant, NOT `Timeouts::teardown`: teardown
/// is the graceful-shutdown-escalation budget, and a caller that sets it to zero (or
/// a tiny value) to demand immediate escalation would otherwise make `resolve`'s
/// bounded worker time out before it could even stat a local directory. Generous
/// enough for a working (even networked) mount, short enough to bound a stalled one.
const RESOLVE_FS_BOUND: Duration = Duration::from_secs(5);

/// Run a blocking FS operation on a DETACHED OS thread, bounded by `budget`. Returns
/// `None` on timeout OR a thread-creation failure. `resolve` runs synchronously,
/// possibly BEFORE any Tokio runtime exists, so this uses a plain `std::thread` +
/// `mpsc::recv_timeout` (no async): `create_dir_all`/`canonicalize` on a stalled
/// NFS/FUSE mount would otherwise block the calling application indefinitely, before
/// any of the supervisor's timeouts or detached workers are established. The worker
/// thread may leak on a wedged mount (like the portfile/probe workers), but the
/// caller returns a bounded error instead of hanging. NOT `spawn_blocking` — there
/// may be no runtime, and a runtime would join its pool on drop.
fn bounded_fs<T, F>(budget: Duration, work: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::sync_channel::<T>(1);
    if std::thread::Builder::new()
        .name("jeliya-resolve-fs".to_owned())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .is_err()
    {
        return None;
    }
    rx.recv_timeout(budget).ok()
}

impl Supervisor {
    /// Resolve the data dir (created and canonicalized, fail-closed) and attempt
    /// binary resolution best-effort. Binary resolution does **not** fail here:
    /// a pure adopter calls only [`Supervisor::attach_to_running`], which needs
    /// no binary; [`Supervisor::start_or_adopt`] surfaces `NoBinary` at spawn
    /// time with the full `tried` list.
    pub fn resolve(config: SupervisorConfig) -> Result<Self, SupervisorError> {
        let data_dir = config.data_dir.unwrap_or_else(default_data_dir);
        // Create AND canonicalize the data dir on a BOUNDED detached worker: both are
        // blocking FS calls that hang indefinitely on a stalled NFS/FUSE mount, and
        // `resolve` runs before any of the supervisor's timeouts or detached workers
        // exist — so an unbounded call here would wedge the calling application at
        // startup even though every later portfile/lock op is time-boxed for the same
        // condition. Bounded by `teardown` (the crate's general bounded-op budget).
        // Canonical form so lock/portfile identity compares like-with-like regardless
        // of `/var` vs `/private/var`, symlinks, or path spelling — the daemon
        // canonicalizes too. FAIL CLOSED on any error rather than storing the
        // unverified (possibly relative) path: `resolve` promises a canonical dir, and
        // a fallback path would either mismatch the absolute one jeliyad records
        // (spurious `DataDirMismatch`) or, if relative, let a later CWD change redirect
        // portfile access and spawning elsewhere.
        let budget = RESOLVE_FS_BOUND;
        let work_dir = data_dir.clone();
        let prepared = bounded_fs(budget, move || {
            std::fs::create_dir_all(&work_dir)?;
            work_dir.canonicalize()
        });
        let data_dir = match prepared {
            Some(Ok(canonical)) => canonical,
            Some(Err(e)) => {
                return Err(SupervisorError::PortfileUnreadable {
                    path: data_dir,
                    why: format!("could not create/canonicalize the data dir: {e}"),
                });
            }
            None => {
                return Err(SupervisorError::PortfileUnreadable {
                    path: data_dir,
                    why: format!(
                        "creating the data dir exceeded the {budget:?} bound (data dir on a stalled mount?)"
                    ),
                });
            }
        };
        // Reject a non-UTF-8 data-dir path up front. jeliyad records the path in
        // its portfile via `display().to_string()`, which replaces non-UTF-8
        // bytes lossily, so the recorded string could never round-trip to equal
        // this canonical path — the data-dir binding would perpetually mismatch
        // and no daemon here could ever be adopted. Fail closed with a clear
        // reason instead of a confusing never-adopt loop.
        if data_dir.to_str().is_none() {
            return Err(SupervisorError::PortfileUnreadable {
                path: data_dir.clone(),
                why: "data dir path is not valid UTF-8; jeliyad records it lossily, so it can never round-trip for adoption".to_owned(),
            });
        }

        // Binary resolution is ALSO blocking FS work — `is_file()` (a stat),
        // `canonical_binary` (canonicalize), and `resolve_jeliyad`'s candidate probes
        // all touch the filesystem — so an explicit binary, `JELIYAD_BIN`, or the
        // bundled executable dir on a stalled mount would hang `resolve` just like the
        // data dir did. Run it on the same BOUNDED worker; a timeout yields "no binary"
        // (recorded in `tried`), which `start_or_adopt` surfaces as `NoBinary` while a
        // pure adopter (`attach_to_running`, no binary needed) still proceeds.
        let explicit = config.binary;
        let (binary, binary_tried) = bounded_fs(RESOLVE_FS_BOUND, move || match explicit {
            // Bind to the ABSOLUTE canonical path, not the caller's (possibly
            // relative) spelling: a relative path stored here re-resolves against
            // the CWD at `start_or_adopt` time, which the process may have changed
            // since — spawning a different file, or failing, despite resolution
            // having succeeded. `canonical_binary` also follows symlinks to the
            // real target it validated.
            Some(explicit) if explicit.is_file() => match canonical_binary(&explicit) {
                Some(canonical) => (Some(canonical), Vec::new()),
                None => (
                    None,
                    vec![format!(
                        "explicit binary {} (could not canonicalize)",
                        explicit.display()
                    )],
                ),
            },
            Some(explicit) => (
                None,
                vec![format!("explicit binary {}", explicit.display())],
            ),
            None => match resolve_jeliyad() {
                Ok(path) => (Some(path), Vec::new()),
                Err(tried) => (None, tried),
            },
        })
        .unwrap_or_else(|| {
            (
                None,
                vec![
                    "binary resolution exceeded its bound (an executable path on a stalled mount?)"
                        .to_owned(),
                ],
            )
        });

        Ok(Self {
            binary,
            binary_tried,
            data_dir,
            expected: config.expected,
            loopback: config.loopback,
            replace_incompatible: config.replace_incompatible,
            strict_portfile_perms: config.strict_portfile_perms,
            timeouts: config.timeouts,
        })
    }

    /// The canonical data dir this supervisor manages.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Spawn a daemon for the data dir, or adopt the one already serving it.
    /// Blocks until the daemon has announced itself and passed validation, so an
    /// `Ok` sidecar is bound and answering — not still starting.
    pub async fn start_or_adopt(&self) -> Result<Sidecar, SupervisorError> {
        // Gate the initial entry: a stale incompatible portfile is refused before
        // spawning over it. The gate itself defers to the eviction path only for
        // a genuinely LIVE incompatible incumbent under `replace_incompatible`
        // (see `start_or_adopt_inner`); the eviction respawn passes `false`.
        self.start_or_adopt_inner(self.replace_incompatible, true)
            .await
    }

    fn start_or_adopt_inner<'a>(
        &'a self,
        allow_evict: bool,
        gate_incompatible: bool,
    ) -> crate::error::BoxFuture<'a, Result<Sidecar, SupervisorError>> {
        // Boxed so the one-shot evict→respawn recursion has a concrete future
        // type (async fns cannot recurse without indirection).
        Box::pin(async move {
            let binary = self
                .binary
                .clone()
                .ok_or_else(|| SupervisorError::NoBinary {
                    tried: self.binary_tried.clone(),
                })?;

            // Gate an INCOMPATIBLE existing portfile BEFORE spawning. If the data
            // dir holds a v1 (or otherwise incompatible) `daemon.json`, spawning
            // first lets the fresh v2 daemon initialize and OVERWRITE it with v2
            // fields before we ever read it — so validation would see the
            // replacement and succeed, silently opening v1 storage and discarding
            // the clean-slate reset the mismatch must trigger.
            //
            // The refusal fires for a STALE incompatible portfile REGARDLESS of
            // `replace_incompatible`: with no live daemon there is nothing to
            // evict, so `replace_incompatible` alone must NOT license a silent
            // overwrite. It is skipped ONLY when the incumbent is genuinely LIVE
            // and the caller opted into replacement — then the spawn reaches the
            // eviction path (`already_running` → prove-owned → SIGTERM →
            // respawn), which is where a live incompatible incumbent is replaced.
            // (`gate_incompatible` is also false on the eviction respawn itself.)
            //
            // Deferring here does NOT special-case the post-spawn `ready` path. If
            // the proven-live incumbent exits in the window between the prove and
            // the spawn, our own fresh daemon acquires the dir and announces
            // `ready`. That daemon is the bundled binary — it serves THIS build's
            // generation — so `finish_owned`'s served-generation gate
            // (`validate_portfile` step 4) governs it exactly as it governs any
            // owned spawn: a match adopts it (the incumbent evicted itself; the end
            // state is §6.7 case 13's "new daemon matches expected generation"), a
            // drift fails closed. The supervisor gates on the SERVED generation and
            // never lays out or migrates on-disk state (spec §12; that is #185), so
            // there is no additional "incompatible storage" invariant for it to
            // enforce beyond that served-generation gate.
            if gate_incompatible {
                match portfile::read_portfile_bounded(&self.data_dir, self.strict_portfile_perms)
                    .await
                {
                    // No portfile → nothing to gate; proceed to spawn.
                    Err(SupervisorError::PortfileMissing(_)) => {}
                    // A truncated / malformed / permission-denied portfile is
                    // EVIDENCE, not absence — silently spawning over it discards
                    // it. Fail closed with the read error (the eviction respawn,
                    // gate_incompatible=false, is unaffected).
                    Err(e) => return Err(e),
                    Ok(existing) => {
                        let declared = existing.declared_generation();
                        if !self.expected.matches(declared) {
                            // Defer to eviction ONLY for a live incumbent bound to
                            // THIS directory. A COPIED portfile recording a foreign
                            // data_dir whose original daemon is still live would
                            // otherwise `prove_owned` true and get the ORIGINAL
                            // SIGTERMed — so the data-dir binding gates the
                            // replaceable check, not just the dial path.
                            let live_replaceable = self.replace_incompatible
                                && validate::data_dir_mismatch(&self.data_dir, &existing.data_dir)
                                    .is_none()
                                && validate::prove_owned(&existing, &self.timeouts).await;
                            if !live_replaceable {
                                return Err(SupervisorError::GenerationMismatch {
                                    expected: self.expected,
                                    actual: declared,
                                });
                            }
                        }
                    }
                }
            }

            let (mut guard, stdin, mut lines) = self.spawn(&binary).await?;
            // `guard` keeps the spawned child force-killable through the whole
            // handshake: if this future is dropped before an owned `Sidecar` takes
            // ownership, the guard SIGKILLs the child's group (its stdin-EOF
            // parent-death signal only covers a well-behaved daemon).
            let announced = match read_announcement(&mut lines, self.timeouts.spawn).await {
                Ok(line) => line,
                Err(AnnouncementError {
                    error,
                    stdout_closed,
                }) => {
                    // No announcement. `Wedged` (retryable "starting, retry in a
                    // moment") is justified ONLY when stdout CLOSED with no line
                    // (`stdout_closed`) AND the child exited nonzero — jeliyad's
                    // `wait_for_free_lock` exits 1 silently on a held lock, and it ALSO
                    // exits 1 silently on a genuine startup failure (lockfile/token/
                    // engine/limits/portfile), which the exit alone cannot distinguish;
                    // `Wedged` is the safe default for that SILENT pair (a held lock
                    // clears on retry, a persistent failure exhausts the retry budget and
                    // surfaces). A NON-EOF failure (malformed JSON, a read error) is a
                    // SPECIFIC handshake fault — masking it as `Wedged` would drive a
                    // caller into futile retry loops over a persistent packaging/protocol
                    // bug — so it is preserved even on a nonzero exit. Drop our stdin
                    // first so a well-behaved daemon can exit on EOF.
                    drop(stdin);
                    // Capture the pgid (== leader pid) BEFORE any reaping wait:
                    // once the leader is reaped, `child.id()` is None and the
                    // isolated group can no longer be reached by pid. A faulty or
                    // overridden binary may have spawned a descendant into this
                    // group with its stdio closed and then exited; the reaped-leader
                    // arms must sweep the group so no descendant keeps holding the
                    // data-dir lock (or writing state) while the caller retries. The
                    // `abandon_child` arms leave the child un-reaped, so
                    // `force_kill_tree` still reaches the group via `child.id()`.
                    let leader_pgid = guard.as_mut().id();

                    if stdout_closed {
                        // EOF: stdout closed with no line. For a single-process daemon
                        // the leader has exited (or is exiting) and no descendant still
                        // holds fd 1 — an inherited-and-open fd 1 would keep the pipe
                        // from reporting EOF — so the exit status is IMMINENT. AWAIT it,
                        // BOUNDED by `teardown`, instead of sampling `try_wait` once: the
                        // single nonblocking poll RACES the reap. The async reader can
                        // deliver EOF a hair before `waitpid(WNOHANG)` sees the zombie,
                        // which mis-routed a silent nonzero exit into `abandon_child` and
                        // surfaced `Handshake("stdout closed before the announcement")`
                        // instead of the retryable `Wedged` (#277). `wait()` is woken by
                        // the runtime's child reaper the instant the zombie is reapable,
                        // so it does NOT burn the budget — the `teardown` ceiling only
                        // bounds a pathological "closed fd 1 but kept running" binary.
                        // `deadline_from` SATURATES so a `Duration::MAX` teardown cannot
                        // overflow-panic the timer.
                        match tokio::time::timeout_at(
                            validate::deadline_from(self.timeouts.teardown),
                            guard.as_mut().wait(),
                        )
                        .await
                        {
                            Ok(Ok(status)) => {
                                if let Some(pgid) = leader_pgid {
                                    // Propagate a verified group-cleanup failure: a
                                    // descendant surviving SIGKILL (still holding the
                                    // data-dir lock) must not be masked by `Wedged`/the
                                    // spawn error, which invite a retry that would wedge.
                                    process::kill_reaped_process_group(pgid).await?;
                                }
                                // A silent nonzero exit is the retryable held-lock /
                                // startup-failure `Wedged` (spec §6.5); a clean (zero)
                                // exit keeps the original handshake error.
                                if !status.success() {
                                    return Err(SupervisorError::Wedged);
                                }
                                return Err(error);
                            }
                            // The child closed stdout but did not exit within `teardown`
                            // (overridden/pathological), or the wait itself errored:
                            // force-kill the group and surface the handshake fault — NOT
                            // the simple retryable held-lock case, so it is not masked as
                            // `Wedged`.
                            _ => return Err(abandon_child(guard.as_mut(), error).await),
                        }
                    }

                    // A NON-EOF fault (malformed JSON or a read error) is a SPECIFIC
                    // handshake fault: the announcement deadline ALREADY expired inside
                    // `read_announcement`, so do NOT grant a SECOND full `Timeouts::spawn`
                    // here — a hung or overridden daemon that emitted no line AND ignores
                    // stdin-EOF would otherwise push `start_or_adopt` to ~2x the
                    // configured bound. A single NONBLOCKING `try_wait` reaps a child that
                    // already exited without extending a hung one; the error is preserved
                    // regardless of the exit status so a persistent packaging/protocol bug
                    // is NOT masked as retryable `Wedged`.
                    match guard.as_mut().try_wait() {
                        Ok(Some(_)) => {
                            if let Some(pgid) = leader_pgid {
                                // Propagate a verified group-cleanup failure: a
                                // descendant surviving SIGKILL (still holding the
                                // data-dir lock) must not be masked by the spawn error,
                                // which invites a retry that would wedge.
                                process::kill_reaped_process_group(pgid).await?;
                            }
                            return Err(error);
                        }
                        // Still running (`Ok(None)`) or an errored wait: force-kill the
                        // group now, bounded, rather than waiting.
                        _ => return Err(abandon_child(guard.as_mut(), error).await),
                    }
                }
            };

            // The portfile is written before the announcement, so it is readable
            // the instant the line parses.
            let portfile =
                match portfile::read_portfile_bounded(&self.data_dir, self.strict_portfile_perms)
                    .await
                {
                    Ok(pf) => pf,
                    Err(e) => return Err(abandon_child(guard.as_mut(), e).await),
                };

            match announced {
                ReadyLine::Ready { pid, port } => {
                    // Our own fresh daemon announced itself. `finish_owned`'s
                    // served-generation gate governs it — including the deferred
                    // race where a proven-live incompatible incumbent exited before
                    // this spawn: the fresh daemon serves this build's generation,
                    // so validation adopts it (§6.7 case 13) or fails closed on a
                    // real drift, never on the vanished incumbent's stale portfile.
                    self.finish_owned(guard, stdin, portfile, pid, port).await
                }
                ReadyLine::AlreadyRunning { pid, port } => {
                    // Our spawned child bowed out with exit 0; the incumbent is
                    // the real one. Drop our stdin so the exiting child is not
                    // held open, then await its exit — BOUNDED. Every wait in
                    // this crate is time-boxed (spec §7.7): a mismatched or
                    // fault-injected binary that prints `already_running` and
                    // then hangs instead of exiting must not wedge
                    // `start_or_adopt` forever despite `Timeouts::spawn`. On
                    // expiry, force-kill the owned probe child (we spawned it)
                    // and surface `Wedged`.
                    drop(stdin);
                    // Capture the pgid BEFORE the reaping wait: our probe child
                    // bowed out, but a faulty/overridden binary could have spawned
                    // a descendant into the isolated group first. The reaped arms
                    // (adopt, or `Wedged`) must sweep the group so nothing lingers
                    // on the data-dir lock; the `abandon_child` arms leave the child
                    // un-reaped, so `force_kill_tree` already reaches it.
                    let leader_pgid = guard.as_mut().id();
                    // Anchor an absolute deadline: `timeout` polls the ready `wait()`
                    // before its timer, so a probe-child exit observed only AT/AFTER the
                    // budget (executor starvation, or `Timeouts::spawn == 0`) must not
                    // slip into the ADOPT path — treat it as expired, like the announcement
                    // and spawn-result paths.
                    let wait_deadline = validate::deadline_from(self.timeouts.spawn);
                    match tokio::time::timeout(self.timeouts.spawn, guard.as_mut().wait()).await {
                        Ok(Ok(status)) if status.success() => {
                            // Our probe child bowed out cleanly; we now fall through
                            // to ADOPT the incumbent — but ONLY if the exit was within
                            // budget. Confirm the isolated group is gone FIRST — a leaked
                            // descendant could still hold the data-dir lock and wreck the
                            // adopt — propagating a bounded cleanup failure instead of
                            // adopting over it.
                            if let Some(pgid) = leader_pgid {
                                process::kill_reaped_process_group(pgid).await?;
                            }
                            // A clean exit observed only past the deadline is out-of-budget:
                            // do NOT adopt (the wait was not actually deadline-strict), fail
                            // `Wedged` so the caller's bounded retry governs.
                            if tokio::time::Instant::now() >= wait_deadline {
                                return Err(SupervisorError::Wedged);
                            }
                        }
                        Ok(Ok(_status)) => {
                            // Already returning `Wedged`; await the bounded sweep so
                            // we do not return over a live subtree, but the primary
                            // error stands.
                            if let Some(pgid) = leader_pgid {
                                // Propagate a verified group-cleanup failure: a
                                // descendant surviving SIGKILL (still holding the
                                // data-dir lock) must not be masked by `Wedged`/the
                                // spawn error, which invite a retry that would wedge.
                                process::kill_reaped_process_group(pgid).await?;
                            }
                            return Err(SupervisorError::Wedged);
                        }
                        Ok(Err(e)) => {
                            return Err(abandon_child(
                                guard.as_mut(),
                                SupervisorError::Handshake(format!(
                                    "adopted-path child never exited: {e}"
                                )),
                            )
                            .await)
                        }
                        Err(_elapsed) => {
                            return Err(abandon_child(guard.as_mut(), SupervisorError::Wedged).await)
                        }
                    }
                    self.finish_adopted(portfile, pid, port, allow_evict).await
                }
            }
        })
    }

    /// Validate an owned (`ready`) daemon and wrap it as a [`Sidecar`].
    async fn finish_owned(
        &self,
        mut guard: process::SpawnGuard,
        stdin: Option<ChildStdin>,
        portfile: Portfile,
        ready_pid: u32,
        ready_port: u16,
    ) -> Result<Sidecar, SupervisorError> {
        // Hold the child under `guard` through this whole validation: `into_child`
        // is called ONLY at the `owned_sidecar` hand-off, so a cancel during the
        // health/PID gate below still SIGKILLs the child's group instead of leaking
        // a bare child (whose stdin-EOF signal an overridden daemon could ignore).
        //
        // The announced PID must be OUR spawned child's PID. A faulty or
        // overridden binary could print a `ready` line quoting the PID of an
        // existing compatible healthy daemon (matching its portfile) while being
        // a different process; binding to `child.id()` refuses that impersonation.
        let child_pid = guard.as_mut().id();
        if child_pid != Some(ready_pid) {
            return Err(abandon_child(
                guard.as_mut(),
                SupervisorError::Handshake(format!(
                    "ready line announced pid {ready_pid}, but the spawned child is pid {child_pid:?} — the binary announced a PID that is not itself"
                )),
            )
            .await);
        }
        if portfile.pid != ready_pid || portfile.port != ready_port {
            return Err(abandon_child(
                guard.as_mut(),
                SupervisorError::Handshake(format!(
                    "ready line says pid {ready_pid} port {ready_port} but the portfile says pid {} port {}",
                    portfile.pid, portfile.port
                )),
            )
            .await);
        }
        // Full agreement gate (loopback, declared+served generation, health/PID).
        // Owned: we stop by closing stdin, never by an adopted lock probe, so do NOT
        // capture the `daemon.lock` handle (a stalled lock manager must not delay an
        // owned start for a handle `owned_sidecar` discards).
        match validate::validate_portfile(
            &self.data_dir,
            self.expected,
            self.strict_portfile_perms,
            &self.timeouts,
            false,
        )
        .await
        {
            Ok(validated) => {
                // `validate_portfile` RE-READS the portfile, so it can observe a
                // DIFFERENT daemon than the one we spawned: if our child exited
                // right after its ready line and another launcher wrote a fresh
                // portfile before this re-read, `validated.portfile` would carry
                // the replacement's PID/port while `child`/`stdin` still refer to
                // our original process. Marking that Owned would let `shutdown`
                // signal only the dead original and leave the real daemon
                // running. Bind the re-read identity back to OUR child's ready
                // announcement; a drift means we no longer own the serving
                // daemon, so refuse rather than mispair (the P2 the review
                // names).
                if validated.portfile.pid != ready_pid || validated.portfile.port != ready_port {
                    let mismatch = SupervisorError::Handshake(format!(
                        "owned child announced pid {ready_pid} port {ready_port} but the validated portfile now serves pid {} port {} — a replacement daemon raced our spawn",
                        validated.portfile.pid, validated.portfile.port
                    ));
                    return Err(abandon_child(guard.as_mut(), mismatch).await);
                }
                // Lifecycle ownership transfers to the `Sidecar` here — disarm the
                // guard now that a real owner exists.
                Ok(self.owned_sidecar(guard.into_child(), stdin, validated.portfile))
            }
            Err(e) => {
                // A daemon WE spawned failed validation — the bundled binary
                // drifted from `expected` (R6). Stop it (we own it) and surface
                // the error rather than leaving a mispaired daemon running.
                Err(abandon_child(guard.as_mut(), e).await)
            }
        }
    }

    /// Validate an adopted (`already_running`) incumbent, evicting it first if it
    /// is a proven-owned incompatible daemon and eviction is opted in.
    async fn finish_adopted(
        &self,
        portfile: Portfile,
        announced_pid: u32,
        announced_port: u16,
        allow_evict: bool,
    ) -> Result<Sidecar, SupervisorError> {
        if portfile.pid != announced_pid || portfile.port != announced_port {
            return Err(SupervisorError::Handshake(format!(
                "already_running says pid {announced_pid} port {announced_port} but the portfile says pid {} port {}",
                portfile.pid, portfile.port
            )));
        }
        // Adopted: `stop_adopted` proves the daemon's exit against the `daemon.lock`
        // inode, so capture (retain) the handle here.
        match validate::validate_portfile(
            &self.data_dir,
            self.expected,
            self.strict_portfile_perms,
            &self.timeouts,
            true,
        )
        .await
        {
            Ok(validated) => Ok(self.adopted_sidecar(validated.portfile, validated.adopted_lock)),
            Err(SupervisorError::GenerationMismatch { expected, actual }) if allow_evict => {
                // Skew path (spec §6.7): replace ONLY a proven-owned incumbent.
                // Re-prove ownership independently (the validation short-circuited
                // on the declared mismatch before its own health step), so a
                // recycled/dead PID or foreign process is never signalled.
                //
                // Residual window (accepted): there is an unavoidable TOCTOU gap
                // between `prove_owned` and the SIGTERM — the proven PID could, in
                // principle, exit and be recycled onto an unrelated same-uid
                // process in that window. The blast radius is bounded by the
                // loopback threat model: the signal carries this process's uid, so
                // a cross-uid victim gets EPERM, and a same-uid attacker could
                // `kill` directly anyway. Closing it fully needs a pidfd
                // (Linux-only) or a `/proc/<pid>/comm` recheck (not portable to
                // the macOS target), so it is documented rather than partially
                // mitigated. `SignalPid` still guarantees the value itself is a
                // representable positive PID (never a group broadcast).
                //
                // Bind the ownership proof AND the SIGTERM to a FRESH single
                // snapshot, not the outer `portfile`: `validate_portfile` re-read
                // the file internally, so the mismatch may describe a portfile that
                // was concurrently REPLACED after the outer read. Signalling
                // `portfile.pid` here could then SIGTERM a still-live COMPATIBLE
                // incumbent that the (now-replaced) mismatch never described. Re-read
                // once and evict only if THAT snapshot is itself an incompatible
                // incumbent bound to this dir whose HEALTH confirms an incompatible
                // served generation (`prove_owned_incompatible`), so a portfile
                // corrupted to merely DECLARE incompatible cannot doom a daemon that
                // actually serves a supported generation.
                let fresh = match portfile::read_portfile_bounded(
                    &self.data_dir,
                    self.strict_portfile_perms,
                )
                .await
                {
                    Ok(pf) => pf,
                    // The portfile vanished/tore under us — nothing proven to evict.
                    Err(_) => return Err(SupervisorError::GenerationMismatch { expected, actual }),
                };
                // Evict iff the fresh snapshot is bound to THIS dir and its HEALTH
                // proves both the PID and an incompatible SERVED generation
                // (`prove_owned_incompatible`). Do NOT additionally require the
                // DECLARATION to mismatch: a daemon whose portfile declares the
                // expected generation but whose health advertises an incompatible
                // one (the inverse portfile/health skew) is just as un-adoptable,
                // and the served proof is what makes eviction safe. The declaration
                // adds nothing — a portfile corrupted to merely DECLARE incompatible
                // is already refused here because its health serves a SUPPORTED
                // generation (`prove_owned_incompatible` returns false).
                let bound_to_dir =
                    validate::data_dir_mismatch(&self.data_dir, &fresh.data_dir).is_none();
                if bound_to_dir
                    && validate::prove_owned_incompatible(&fresh, self.expected, &self.timeouts)
                        .await
                {
                    process::sigterm_foreign(fresh.pid)?;
                    if validate::wait_health_dark(
                        fresh.pid,
                        fresh.port,
                        self.timeouts.evict,
                        &self.timeouts,
                    )
                    .await
                    {
                        // Respawn exactly once (no further eviction), against the
                        // now-free data dir.
                        self.start_or_adopt_inner(false, false).await
                    } else {
                        Err(SupervisorError::ShutdownTimedOut { pid: fresh.pid })
                    }
                } else {
                    // Unprovable (fault #14), drifted, or the live daemon actually
                    // serves a compatible generation: fail closed, signal nothing.
                    Err(SupervisorError::GenerationMismatch { expected, actual })
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Attach-only: adopt a running daemon from its portfile alone (no spawn, no
    /// binary needed). For a second native client riding along a daemon someone
    /// else supervises. Runs the loopback / health-PID / generation gate first.
    pub async fn attach_to_running(&self) -> Result<Sidecar, SupervisorError> {
        // Adopt path: retain the `daemon.lock` handle for `stop_adopted`.
        let validated = validate::validate_portfile(
            &self.data_dir,
            self.expected,
            self.strict_portfile_perms,
            &self.timeouts,
            true,
        )
        .await?;
        Ok(self.adopted_sidecar(validated.portfile, validated.adopted_lock))
    }

    fn owned_sidecar(
        &self,
        child: Child,
        stdin: Option<ChildStdin>,
        portfile: Portfile,
    ) -> Sidecar {
        Sidecar {
            portfile,
            ownership: Ownership::Owned { child, stdin },
            data_dir: self.data_dir.clone(),
            expected: self.expected,
            strict_portfile_perms: self.strict_portfile_perms,
            timeouts: self.timeouts,
            // An owned daemon is stopped by closing its stdin (`shutdown`), never by
            // an adopted lock probe, so no lock handle is retained.
            adopted_lock: None,
        }
    }

    fn adopted_sidecar(
        &self,
        portfile: Portfile,
        adopted_lock: Option<fd_lock::RwLock<std::fs::File>>,
    ) -> Sidecar {
        // The `daemon.lock` handle was captured as the FINAL step of
        // `validate_portfile`, bound to the same identity the health probe proved,
        // and is retained here so `stop_adopted` proves the daemon's exit against
        // THAT inode — not one re-opened at shutdown (which a cleanup tool could have
        // unlinked/replaced with an unrelated process's lock). `None` (capture
        // missed) fails closed in `stop_adopted`.
        Sidecar {
            portfile,
            ownership: Ownership::Adopted,
            data_dir: self.data_dir.clone(),
            expected: self.expected,
            strict_portfile_perms: self.strict_portfile_perms,
            timeouts: self.timeouts,
            adopted_lock,
        }
    }

    /// Spawn `jeliyad --supervised --data-dir <dir> --port 0 [--loopback]` with
    /// stdin/stdout/stderr piped, `kill_on_drop(false)` (the stdin pipe, not
    /// `Drop`, is the parent-death mechanism), and the child in its own process
    /// group. Drains stderr forever on a background task — an unread full stderr
    /// pipe deadlocks the daemon's synchronous tracing layer (the same records
    /// survive in `<data_dir>/logs`).
    async fn spawn(&self, binary: &Path) -> Result<SpawnedDaemon, SupervisorError> {
        // The data dir is already created AND canonicalized by `Supervisor::resolve`
        // (fail-closed there), so no `create_dir_all` here: a second one would be a
        // redundant synchronous `stat`+`mkdir` on the executor thread that, on a
        // stalled NFS/FUSE mount, could hang `start_or_adopt` indefinitely — and on
        // a current-thread runtime wedge every other task — BEFORE the announcement
        // timeout is even established. The daemon (`--data-dir`) recreates the dir if
        // it vanished after resolve.

        let mut cmd = Command::new(binary);
        cmd.arg("--supervised")
            .arg("--data-dir")
            .arg(&self.data_dir)
            .arg("--port")
            .arg("0");
        if self.loopback {
            cmd.arg("--loopback");
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // See the module note on the spike: false is deliberate — the stdin
            // pipe is the only thing between a dead shell and an orphaned daemon,
            // which is exactly the production situation.
            .kill_on_drop(false);
        process::configure_new_process_group(&mut cmd);

        // Retry a transient ETXTBSY ("Text file busy", errno 26). A binary that
        // was just written — a freshly installed/updated jeliyad, or (in the
        // tests) a stub whose write-fd is momentarily held by a sibling
        // process's fork before its exec — cannot be exec'd until that writer
        // fd is gone. It clears within milliseconds, so a short bounded backoff
        // turns a spurious hard failure into a successful spawn.
        //
        // Each `spawn()` runs OFF the executor via `spawn_blocking`, bounded by the
        // spawn deadline: `Command::spawn` is synchronous and, because fork+exec
        // reports exec failure back over a pipe, it BLOCKS the caller until the
        // child execs — so an executable on a stalled NFS/FUSE mount would hang this
        // thread (wedging a current-thread runtime) BEFORE `read_announcement`
        // establishes the spawn timeout. On timeout we detach a cleanup task that
        // awaits the still-running spawn and kills any child a late exec produces,
        // so a stalled spawn never leaks a daemon.
        let spawn_deadline = validate::deadline_from(self.timeouts.spawn);
        let mut guard = {
            let mut attempts = 0u32;
            loop {
                let remaining =
                    spawn_deadline.saturating_duration_since(tokio::time::Instant::now());
                // The spawn runs on a blocking thread and HANDS its outcome back over
                // a oneshot. The blocking worker OWNS cleanup on the timeout path: if
                // we time out and drop the receiver, its `send` fails and it SIGKILLs
                // the late child's whole group itself (via `nix`, valid even with no
                // runtime), so a late exec (dropped with `kill_on_drop(false)`) cannot
                // leak its group even if the caller drops the runtime. The oneshot
                // makes the hand-off race-free: exactly one of {we receive the child,
                // the worker cleans it up} happens.
                //
                // The worker runs on a DETACHED OS THREAD, not `spawn_blocking`:
                // `Command::spawn` blocks until fork+exec reports back, so an
                // executable on a stalled NFS/FUSE mount can wedge it INDEFINITELY, and
                // Tokio JOINS its blocking pool on `Runtime::drop` — so a `spawn_blocking`
                // worker would turn the caller-visible timeout into an application-
                // shutdown hang. A plain `std::thread` is not joined by the runtime, so
                // a wedged spawn cannot hang shutdown (the OS reaps the thread at process
                // exit). It ENTERS a runtime `Handle` so `tokio::process` spawn (which
                // needs the process reactor) still works.
                //
                // The worker also STAMPS whether the spawn completed at/after the
                // deadline against a std-clock deadline it can read (it cannot use
                // `tokio::time`). `timeout` polls the ready receiver BEFORE its timer,
                // so a spawn finishing just past the budget would otherwise be accepted
                // and startup would continue past the deadline; the stamp lets us
                // REJECT and clean up such a late child instead.
                let produced_deadline = std::time::Instant::now() + remaining;
                let (tx, rx) = tokio::sync::oneshot::channel::<(
                    Command,
                    std::io::Result<process::SpawnGuard>,
                    bool,
                )>();
                let rt_handle = tokio::runtime::Handle::current();
                let worker = std::thread::Builder::new()
                    .name("jeliya-daemon-spawn".to_owned())
                    .spawn(move || {
                        let _rt = rt_handle.enter();
                        // `cmd.spawn()` is `std.spawn()?` then a fallible Tokio
                        // registration (stdio → non-blocking `PollEvented`, and the
                        // SIGCHLD reaper). If the CALLER dropped its runtime in the
                        // microsecond after the OS child was created, that registration
                        // can fail and Tokio's `Command::spawn` drops the bare
                        // `std::process::Child` — we get an `Err` with NO handle, so no
                        // pid to guard. There is no leak of the data-dir lock for the
                        // daemon we ship: the child was spawned with `stdin(piped())`,
                        // and that dropped write-end delivers stdin-EOF, which a
                        // `--supervised` jeliyad treats as its shutdown signal (the same
                        // parent-death mechanism `kill_on_drop(false)` relies on). Only a
                        // child that IGNORES stdin-EOF could persist, and reaching it
                        // needs a pid Tokio never exposes — closing that would require
                        // re-implementing Tokio's private `build_child` (SIGCHLD reaper +
                        // `PollEvented` stdio) or `pre_exec`, both barred by
                        // `unsafe_code = "forbid"`. Documented residual, not a fix we can
                        // safely reach.
                        let result = cmd.spawn().map(process::SpawnGuard::new);
                        let late = std::time::Instant::now() >= produced_deadline;
                        // Hand the outcome over. The `SpawnGuard` tears the child's
                        // group down on ANY drop it is not consumed by: a oneshot
                        // `send` acknowledges only that the value was QUEUED, not that
                        // the caller consumed it, so if the receiver is gone (the caller
                        // timed out, OR aborted after a successful queue) the returned
                        // tuple drops here and the guard cleans up — the send RESULT is
                        // not what makes a late child cannot-leak, the guard is.
                        let _ = tx.send((cmd, result, late));
                    });
                // `Builder::spawn` returns an error (unlike `thread::spawn`, which
                // PANICS) when the OS refuses another thread under resource pressure,
                // so daemon startup surfaces `Spawn` instead of unwinding/aborting the
                // supervising application.
                if let Err(e) = worker {
                    return Err(SupervisorError::Spawn(std::io::Error::new(
                        e.kind(),
                        format!("could not start the process-creation worker thread: {e}"),
                    )));
                }
                let (returned, result) = match tokio::time::timeout(remaining, rx).await {
                    Ok(Ok((spawned_cmd, spawn_result, late))) => {
                        let result = match spawn_result {
                            Ok(guard) if late => {
                                // The spawn completed only at/after the deadline: reject
                                // it and tear the child down so startup never proceeds
                                // past the budget with a late-created daemon. Fold a
                                // cleanup FAILURE (the subtree survived SIGKILL past the
                                // grace window, still holding the data-dir lock) into the
                                // error so the caller does not retry over a surviving one.
                                let mut child = guard.into_child();
                                let mut detail =
                                    "process creation completed after the spawn budget expired"
                                        .to_owned();
                                if let Err(cleanup) = process::force_kill_tree(&mut child).await {
                                    detail = format!(
                                        "{detail}; cleanup of the late child failed: {cleanup}"
                                    );
                                }
                                return Err(SupervisorError::Spawn(std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    detail,
                                )));
                            }
                            // RETAIN the guard: startup validation (announcement,
                            // portfile, health/PID gate) runs under it, so a cancel
                            // before an owned `Sidecar` takes ownership still tears
                            // the child's group down. It is disarmed only at the
                            // Sidecar hand-off (`finish_owned` → `owned_sidecar`).
                            Ok(guard) => Ok(guard),
                            Err(e) => Err(e),
                        };
                        (spawned_cmd, result)
                    }
                    Ok(Err(_recv)) => {
                        // The worker dropped the sender without sending — only reachable
                        // if the blocking task panicked before `cmd.spawn()` returned.
                        return Err(SupervisorError::Spawn(std::io::Error::other(
                            "process-creation task ended without an outcome (worker panicked?)",
                        )));
                    }
                    Err(_elapsed) => {
                        // Dropping the timed-out future drops `rx`, so the worker's
                        // `send` fails and it cleans up any late child itself.
                        return Err(SupervisorError::Spawn(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "process creation exceeded the spawn budget (executable on a stalled mount?)",
                        )));
                    }
                };
                cmd = returned;
                match result {
                    Ok(guard) => break guard,
                    Err(e) if e.raw_os_error() == Some(26) && attempts < 10 => {
                        attempts += 1;
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    Err(e) => return Err(SupervisorError::Spawn(e)),
                }
            }
        };
        // `guard` holds the freshly-spawned child; borrow it (never disarm) to take
        // its stdio pipes, so a cancel before the owned `Sidecar` is built still
        // force-kills the group.
        let stdin = guard.as_mut().stdin.take();

        if let Some(mut stderr) = guard.as_mut().stderr.take() {
            tokio::spawn(async move {
                // Drain stderr in fixed RAW-BYTE chunks, not lines: a line reader
                // grows its buffer without bound on an arbitrarily long line, and
                // ERRORS OUT permanently on a non-UTF-8 byte — after which the
                // drain stops and the daemon deadlocks once its stderr pipe fills
                // (the whole reason this task exists). Bytes are discarded; the
                // same records survive in `<data_dir>/logs`.
                let mut buf = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                }
            });
        }

        let stdout = guard
            .as_mut()
            .stdout
            .take()
            .ok_or_else(|| SupervisorError::Handshake("no stdout pipe".to_owned()))?;
        Ok((
            guard,
            stdin,
            BufReader::new(stdout.take(MAX_ANNOUNCEMENT_BYTES)).lines(),
        ))
    }
}

/// The pieces [`Supervisor::spawn`] hands back: the child process, its held
/// stdin (the parent-death pipe, kept for the daemon's life), and a line reader
/// over its stdout for the announcement.
/// The most of a daemon's stdout we will buffer looking for the announcement
/// line: a corrupt/overridden daemon that writes an arbitrarily long line with
/// no newline would otherwise grow `next_line`'s buffer without bound until the
/// spawn timeout (which caps time, not bytes). The real ready line is a few
/// hundred bytes; capping the read at 64 KiB turns a hostile stream into a
/// bounded, unparseable partial that fails the handshake.
const MAX_ANNOUNCEMENT_BYTES: u64 = 64 * 1024;

type SpawnedDaemon = (
    process::SpawnGuard,
    Option<ChildStdin>,
    Lines<BufReader<Take<ChildStdout>>>,
);

/// The daemon's first stdout line. Extra fields (http, ws, version, protocol,
/// storage_generation, limits, data_dir, portfile) are ignored — the generation
/// axes are validated from the portfile and health, not this line.
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum ReadyLine {
    Ready { pid: u32, port: u16 },
    AlreadyRunning { pid: u32, port: u16 },
}

/// Why [`read_announcement`] failed. `stdout_closed` marks the EOF case — stdout
/// closed with NO JSON line, i.e. the daemon exited SILENTLY (a held lock, or a
/// startup failure jeliyad reports only by exiting nonzero without a line). ONLY that
/// case justifies mapping a nonzero exit to the retryable `Wedged`; every other
/// failure (timeout, read error, malformed JSON) is a SPECIFIC handshake fault to
/// preserve, so a persistent packaging/protocol bug is not masked as "retry me".
#[derive(Debug)]
struct AnnouncementError {
    error: SupervisorError,
    stdout_closed: bool,
}

impl AnnouncementError {
    fn stdout_closed() -> Self {
        Self {
            error: SupervisorError::Handshake("stdout closed before the announcement".to_owned()),
            stdout_closed: true,
        }
    }
    fn other(error: SupervisorError) -> Self {
        Self {
            error,
            stdout_closed: false,
        }
    }
}

/// Read the first stdout line that starts with `{` (skipping any stray
/// human-readable output), parse it as a [`ReadyLine`], all within `budget`.
/// Generic over the reader so the deadline behaviour is unit-testable against an
/// in-memory stream (the production caller passes the child's stdout `Lines`).
async fn read_announcement<R>(
    lines: &mut Lines<R>,
    budget: Duration,
) -> Result<ReadyLine, AnnouncementError>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    // Anchor an absolute deadline BEFORE the read loop. `timeout` polls the wrapped
    // future BEFORE its timer, so when a line and the timer are both ready Tokio
    // returns the line even though the budget expired — the same ready-future/timer
    // ordering the process-creation path stamps against. Reject a candidate line
    // observed only AT/AFTER this deadline so startup never continues past the spawn
    // budget on a daemon that announced itself late. `deadline_from` SATURATES, so a
    // caller-supplied `Timeouts::spawn` of `Duration::MAX` cannot overflow-panic the
    // `Instant + Duration` (it becomes effectively unbounded, matching the spawn phase).
    let deadline = validate::deadline_from(budget);
    let line = match tokio::time::timeout(budget, async {
        loop {
            match lines.next_line().await {
                Ok(Some(line)) if line.trim_start().starts_with('{') => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(AnnouncementError::other(SupervisorError::Handshake(
                            "the announcement arrived only after the spawn budget expired"
                                .to_owned(),
                        )));
                    }
                    return Ok(line);
                }
                // Skip a non-JSON line (belt-and-suspenders; the daemon emits the
                // JSON first) and keep reading.
                Ok(Some(_)) => continue,
                // EOF: stdout closed with no JSON line — the ONLY failure that maps a
                // nonzero exit to retryable `Wedged`.
                Ok(None) => return Err(AnnouncementError::stdout_closed()),
                Err(e) => {
                    return Err(AnnouncementError::other(SupervisorError::Handshake(
                        format!("could not read stdout: {e}"),
                    )))
                }
            }
        }
    })
    .await
    {
        Ok(inner) => inner,
        Err(_elapsed) => Err(AnnouncementError::other(SupervisorError::Handshake(
            format!("no announcement within {budget:?}"),
        ))),
    }?;

    serde_json::from_str::<ReadyLine>(&line).map_err(|e| {
        // Do NOT echo the raw line OR the serde error's Display: both reproduce
        // unvalidated, child-controlled content up to MAX_ANNOUNCEMENT_BYTES that a
        // corrupted/overridden daemon could stuff with an `auth_token` or other
        // sensitive value (serde's Display echoes a mis-typed field VALUE, e.g.
        // `invalid type: string "…"`). Callers log or display a Handshake error,
        // so either would bypass the crate's token-redaction boundary (spec §7.2).
        // Report only the CONTENT-FREE classification and position plus the
        // withheld byte count. A malformed announcement is NOT `stdout_closed`, so a
        // nonzero exit alongside it is preserved as this specific fault, not `Wedged`.
        AnnouncementError::other(SupervisorError::Handshake(format!(
            "the daemon's announcement was not valid JSON ({:?} at line {} column {}); {} bytes of raw output withheld (unvalidated child content)",
            e.classify(),
            e.line(),
            e.column(),
            line.len()
        )))
    })
}

/// Force-kill a child we are ABANDONING during a startup failure, folding a kill
/// failure into the surfaced error instead of dropping it: a child that cannot
/// be signalled or reaped (e.g. wedged in an uninterruptible syscall) is a leak
/// worth reporting. When the kill succeeds, the original `error` is returned
/// unchanged.
async fn abandon_child(child: &mut Child, error: SupervisorError) -> SupervisorError {
    match process::force_kill_tree(child).await {
        Ok(()) => error,
        Err(kill_err) => SupervisorError::Handshake(format!(
            "{error} (and the spawned child could not be killed — it may be leaked: {kill_err})"
        )),
    }
}

/// The default per-user data directory the daemon uses
/// (`~/Library/Application Support/Jeliya`, `$XDG_DATA_HOME/Jeliya`, or
/// `%APPDATA%\Jeliya`), falling back to a cwd-relative dir only when no platform
/// path is discoverable — identical to `jeliyad`'s `default_data_dir`.
fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|dir| dir.join("Jeliya"))
        .unwrap_or_else(|| PathBuf::from("./.jeliya-data"))
}

/// Resolve the daemon binary, fail-closed, matching the documented desktop
/// order: (1) `JELIYAD_BIN` if it is a file; (2) a `jeliyad` bundled beside
/// `current_exe()`; (3) **debug builds only**, the repo `target/debug/jeliyad`.
/// An installed daemon on `PATH` is **never** silently used — a release shell
/// that lost its bundled sidecar fails closed rather than pairing a release UI
/// with an unknown daemon (`JELIYAD_BIN` is the explicit opt-in). Returns the
/// list of paths tried on exhaustion.
fn resolve_jeliyad() -> Result<PathBuf, Vec<String>> {
    let mut tried = Vec::new();

    // Every resolved candidate is canonicalized (absolute, symlink-resolved)
    // before it is returned, so a relative `JELIYAD_BIN` cannot re-resolve against
    // a changed CWD at spawn time and the stored path always names the exact file
    // that was validated.
    if let Some(explicit) = std::env::var_os("JELIYAD_BIN") {
        let path = PathBuf::from(explicit);
        if let Some(canonical) = canonical_binary(&path) {
            return Ok(canonical);
        }
        tried.push(format!("JELIYAD_BIN={}", path.display()));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join(bundled_name());
            if let Some(canonical) = canonical_binary(&bundled) {
                return Ok(canonical);
            }
            tried.push(bundled.display().to_string());
        }
    }

    #[cfg(debug_assertions)]
    {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("target/debug")
            .join(bundled_name());
        if let Some(canonical) = canonical_binary(&repo) {
            return Ok(canonical);
        }
        tried.push(repo.display().to_string());
    }
    #[cfg(not(debug_assertions))]
    tried.push("(the target/debug fallback is debug-only)".to_owned());

    Err(tried)
}

/// Canonicalize a candidate daemon path to an ABSOLUTE, symlink-resolved target
/// and confirm it is a file. Returns `None` when the path is not a file or can no
/// longer be canonicalized (racing removal / permissions), so the caller records
/// it as tried rather than storing a relative spelling that would re-resolve
/// against a possibly-changed CWD — or a symlink that could be repointed —
/// between resolution and the eventual `spawn`.
fn canonical_binary(path: &Path) -> Option<PathBuf> {
    path.canonicalize()
        .ok()
        .filter(|resolved| resolved.is_file())
}

/// The daemon's file name for the current platform.
fn bundled_name() -> &'static str {
    if cfg!(windows) {
        "jeliyad.exe"
    } else {
        "jeliyad"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `canonical_binary` binds to an ABSOLUTE, symlink-resolved, `..`-free path
    /// for a real file, so a stored binary path cannot re-resolve against a
    /// changed CWD at spawn time; a non-file yields `None`.
    #[test]
    fn canonical_binary_resolves_a_file_to_an_absolute_path() {
        let dir = std::env::temp_dir().join(format!("sup-canon-{}", std::process::id()));
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let bin = sub.join("jeliyad");
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").unwrap();

        // A non-canonical spelling of the same file (a `..` round-trip).
        let noncanonical = sub.join("..").join("sub").join("jeliyad");
        assert!(
            noncanonical.to_string_lossy().contains(".."),
            "the input must be non-canonical to prove resolution happened"
        );
        let resolved = canonical_binary(&noncanonical).expect("a real file canonicalizes");
        assert!(
            resolved.is_absolute(),
            "resolved path must be absolute: {resolved:?}"
        );
        assert!(
            !resolved.to_string_lossy().contains("/../"),
            "resolved path must be canonical (no ..): {resolved:?}"
        );
        // A path that is not a file yields None (recorded as tried, not stored).
        assert!(canonical_binary(&dir.join("does-not-exist")).is_none());
        assert!(
            canonical_binary(&dir).is_none(),
            "a directory is not a binary"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ready_line_parses_both_verdicts() {
        let ready: ReadyLine = serde_json::from_str(
            r#"{"event":"ready","pid":42,"port":7420,"protocol":2,"storage_generation":2}"#,
        )
        .expect("ready parses");
        assert!(matches!(
            ready,
            ReadyLine::Ready {
                pid: 42,
                port: 7420
            }
        ));

        let adopted: ReadyLine =
            serde_json::from_str(r#"{"event":"already_running","pid":7,"port":9000}"#)
                .expect("already_running parses");
        assert!(matches!(
            adopted,
            ReadyLine::AlreadyRunning { pid: 7, port: 9000 }
        ));
    }

    #[test]
    fn an_unknown_event_is_refused() {
        assert!(
            serde_json::from_str::<ReadyLine>(r#"{"event":"exploded","pid":1,"port":2}"#).is_err()
        );
    }

    #[test]
    fn resolve_with_a_missing_explicit_binary_defers_to_start_time() {
        // A pure adopter can construct a Supervisor even with no binary; the
        // `NoBinary` surfaces only when a spawn is attempted.
        let tmp = std::env::temp_dir().join(format!("jeliya-sup-test-{}", std::process::id()));
        let config = SupervisorConfig {
            data_dir: Some(tmp.clone()),
            binary: Some(PathBuf::from("/definitely/not/a/real/jeliyad")),
            ..SupervisorConfig::new(Generation::new(2, 2))
        };
        let sup = Supervisor::resolve(config).expect("resolve succeeds without a binary");
        assert!(sup.binary.is_none());
        assert!(!sup.binary_tried.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn supervisor_config_new_sets_expected_generation_and_safe_defaults() {
        let config = SupervisorConfig::new(Generation::new(3, 4));
        assert_eq!(config.expected.protocol, 3);
        assert_eq!(config.expected.storage_generation, 4);
        assert!(
            config.data_dir.is_none(),
            "data_dir defaults to None (platform dir)"
        );
        assert!(
            config.binary.is_none(),
            "binary defaults to None (fail-closed resolution)"
        );
        assert!(!config.loopback, "loopback defaults to false");
        assert!(
            !config.replace_incompatible,
            "replace_incompatible defaults to false (spec §6.7)"
        );
        assert!(
            !config.strict_portfile_perms,
            "strict_portfile_perms defaults to false (OQ-3)"
        );
    }

    #[test]
    fn timeouts_default_are_sensible() {
        let t = Timeouts::default();
        // Spawn budget matches the spike's 30s announcement window.
        assert_eq!(t.spawn.as_secs(), 30);
        // Teardown must exceed the daemon's ~10s room-close budget.
        assert!(t.teardown.as_secs() >= 10, "teardown budget must be ≥10s");
        // Health probes must be short enough not to wedge a UI.
        assert!(t.health_connect.as_millis() <= 2000);
        assert!(t.health_read.as_secs() <= 5);
    }

    #[test]
    fn resolve_with_explicit_valid_binary_stores_it() {
        let tmp = std::env::temp_dir().join(format!("jeliya-sup-bin-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        // Create a fake "binary" file so `is_file()` returns true.
        let fake_bin = tmp.join("jeliyad");
        std::fs::write(&fake_bin, b"#!/bin/sh\n").unwrap();
        let config = SupervisorConfig {
            data_dir: Some(tmp.clone()),
            binary: Some(fake_bin.clone()),
            ..SupervisorConfig::new(Generation::new(2, 2))
        };
        let sup = Supervisor::resolve(config).expect("resolve succeeds");
        assert_eq!(sup.binary.as_deref(), Some(fake_bin.as_path()));
        assert!(
            sup.binary_tried.is_empty(),
            "no fallback paths tried when explicit binary is a file"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ready_line_ignores_unknown_extra_fields() {
        let ready: ReadyLine = serde_json::from_str(
            r#"{"event":"ready","pid":1,"port":2,"extra_field":"ignored","protocol":2}"#,
        )
        .expect("parses with extra fields");
        assert!(matches!(ready, ReadyLine::Ready { pid: 1, port: 2 }));
    }

    #[test]
    fn supervisor_exposes_its_data_dir() {
        let tmp = std::env::temp_dir().join(format!("jeliya-sup-dir-{}", std::process::id()));
        let config = SupervisorConfig {
            data_dir: Some(tmp.clone()),
            binary: Some(PathBuf::from("/no/such/binary")),
            ..SupervisorConfig::new(Generation::new(2, 2))
        };
        let sup = Supervisor::resolve(config).expect("resolve succeeds");
        // data_dir() must return a path that starts with `tmp` (may be canonicalized).
        let returned = sup.data_dir();
        assert!(
            returned.starts_with(&tmp) || tmp.starts_with(returned),
            "data_dir() must match the configured dir; got {returned:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A `ready` line that is available but observed only AT/AFTER the anchored
    /// deadline must be REJECTED. With a zero budget the deadline equals the start
    /// instant, so `timeout` — which polls the ready inner future before its timer —
    /// hands the line back even though the budget elapsed; the in-loop deadline check
    /// is what refuses it. Red before the check (returned `Ok(Ready …)`), green after.
    #[tokio::test]
    async fn read_announcement_rejects_a_json_line_observed_at_or_after_the_deadline() {
        let data = b"{\"event\":\"ready\",\"pid\":1,\"port\":2}\n";
        let mut lines = BufReader::new(&data[..]).lines();
        let err = read_announcement(&mut lines, Duration::ZERO)
            .await
            .expect_err("a line observed at/after the deadline must be rejected");
        assert!(
            matches!(&err.error, SupervisorError::Handshake(m) if m.contains("after the spawn budget"))
                && !err.stdout_closed,
            "expected the late-announcement handshake error, got {err:?}"
        );
    }

    /// A caller-supplied `Timeouts::spawn` of `Duration::MAX` must NOT overflow-panic
    /// the deadline anchor (`deadline_from` saturates). Red-before: the anchor was
    /// `Instant::now() + budget`, which panics on `Duration::MAX`; green-after it is
    /// treated as effectively unbounded and the ready line parses.
    #[tokio::test]
    async fn read_announcement_does_not_panic_on_an_unbounded_budget() {
        let data = b"{\"event\":\"ready\",\"pid\":1,\"port\":2}\n";
        let mut lines = BufReader::new(&data[..]).lines();
        let announced = read_announcement(&mut lines, Duration::MAX)
            .await
            .expect("an unbounded budget is effectively unbounded, not a panic");
        assert!(matches!(announced, ReadyLine::Ready { pid: 1, port: 2 }));
    }

    /// The deadline gate rejects only LATE lines: a prompt line within a real budget
    /// (after skipping non-JSON noise) still parses. Guards against the check
    /// over-rejecting valid announcements.
    #[tokio::test]
    async fn read_announcement_accepts_a_prompt_json_line_within_budget() {
        let data = b"human-readable noise\n{\"event\":\"ready\",\"pid\":1,\"port\":2}\n";
        let mut lines = BufReader::new(&data[..]).lines();
        let announced = read_announcement(&mut lines, Duration::from_secs(30))
            .await
            .expect("a prompt line within budget is accepted");
        assert!(matches!(announced, ReadyLine::Ready { pid: 1, port: 2 }));
    }

    /// `stdout_closed` is set ONLY for the EOF failure (stdout closed with no JSON
    /// line), not for a malformed-JSON announcement — the flag the no-announcement path
    /// uses to decide whether a nonzero exit is retryable `Wedged` or a preserved
    /// handshake fault.
    #[tokio::test]
    async fn read_announcement_flags_stdout_closed_only_on_eof() {
        // A malformed JSON line is a SPECIFIC fault, not the EOF case.
        let malformed = b"{ not json }\n";
        let mut lines = BufReader::new(&malformed[..]).lines();
        let err = read_announcement(&mut lines, Duration::from_secs(30))
            .await
            .expect_err("malformed JSON must fail");
        assert!(
            !err.stdout_closed,
            "a malformed announcement is not the stdout-closed EOF case"
        );
        // An empty stream (immediate EOF) IS the stdout-closed case.
        let empty: &[u8] = b"";
        let mut lines = BufReader::new(empty).lines();
        let err = read_announcement(&mut lines, Duration::from_secs(30))
            .await
            .expect_err("EOF with no line must fail");
        assert!(
            err.stdout_closed,
            "stdout closed with no announcement IS the EOF case"
        );
    }

    /// `bounded_fs` returns a prompt operation's result unchanged.
    #[test]
    fn bounded_fs_returns_a_prompt_result() {
        assert_eq!(bounded_fs(Duration::from_secs(5), || 42u32), Some(42));
    }

    /// `bounded_fs` returns `None` PROMPTLY when the operation outruns the budget —
    /// the property that stops `resolve`'s data-dir setup from hanging on a stalled
    /// mount. Red-before-green: before the bounded worker, `resolve` blocked on the
    /// synchronous `create_dir_all`/`canonicalize` with no bound at all.
    #[test]
    fn bounded_fs_times_out_a_stalled_operation() {
        let start = std::time::Instant::now();
        let r: Option<()> = bounded_fs(Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(30));
        });
        assert_eq!(r, None, "a slow op must time out to None");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must return at the bound, not wait for the 30s op"
        );
    }

    /// `resolve`'s data-dir setup uses a DEDICATED bound, not `Timeouts::teardown`.
    /// A caller demanding immediate shutdown escalation (teardown = 0) must still be
    /// able to construct a Supervisor. Red-before-green: reusing teardown = 0 as the
    /// worker's `recv_timeout` made resolve fail `PortfileUnreadable` even for an
    /// existing local dir, before the worker could stat it.
    #[test]
    fn resolve_setup_does_not_reuse_a_zero_teardown_budget() {
        let tmp = std::env::temp_dir().join(format!("jeliya-sup-td0-{}", std::process::id()));
        let config = SupervisorConfig {
            data_dir: Some(tmp.clone()),
            binary: Some(PathBuf::from("/definitely/not/a/real/jeliyad")),
            timeouts: Timeouts {
                teardown: Duration::from_secs(0),
                ..Timeouts::default()
            },
            ..SupervisorConfig::new(Generation::new(2, 2))
        };
        let sup = Supervisor::resolve(config)
            .expect("resolve must succeed with teardown=0 (data-dir setup has its own bound)");
        assert!(sup.data_dir().starts_with(&tmp) || tmp.starts_with(sup.data_dir()));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
