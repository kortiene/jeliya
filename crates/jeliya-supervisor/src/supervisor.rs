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

impl Supervisor {
    /// Resolve the data dir (created and canonicalized, fail-closed) and attempt
    /// binary resolution best-effort. Binary resolution does **not** fail here:
    /// a pure adopter calls only [`Supervisor::attach_to_running`], which needs
    /// no binary; [`Supervisor::start_or_adopt`] surfaces `NoBinary` at spawn
    /// time with the full `tried` list.
    pub fn resolve(config: SupervisorConfig) -> Result<Self, SupervisorError> {
        let data_dir = config.data_dir.unwrap_or_else(default_data_dir);
        std::fs::create_dir_all(&data_dir).map_err(|e| SupervisorError::PortfileUnreadable {
            path: data_dir.clone(),
            why: format!("could not create the data dir: {e}"),
        })?;
        // Canonical form so lock/portfile identity compares like-with-like
        // regardless of `/var` vs `/private/var`, symlinks, or path spelling —
        // the daemon canonicalizes too. FAIL CLOSED on a canonicalize error rather
        // than storing the unverified (possibly relative) path: `resolve` promises
        // a canonical dir, and a fallback path would either mismatch the absolute
        // one jeliyad records (spurious `DataDirMismatch`) or, if relative, let a
        // later CWD change redirect portfile access and spawning elsewhere.
        let data_dir = match data_dir.canonicalize() {
            Ok(canonical) => canonical,
            Err(e) => {
                return Err(SupervisorError::PortfileUnreadable {
                    path: data_dir,
                    why: format!("could not canonicalize the data dir: {e}"),
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

        let (binary, binary_tried) = match config.binary {
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
        };

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

            let (mut child, stdin, mut lines) = self.spawn(&binary).await?;
            let announced = match read_announcement(&mut lines, self.timeouts.spawn).await {
                Ok(line) => line,
                Err(e) => {
                    // No announcement. The most common cause of a non-zero
                    // SILENT exit is the data-dir lock being held with no healthy
                    // daemon (jeliyad's `wait_for_free_lock` exits 1 without a
                    // line), which is the retryable `Wedged` verdict ("starting,
                    // retry in a moment"). jeliyad ALSO exits 1 silently on a
                    // genuine startup failure (lockfile open, token, engine init,
                    // limits, portfile write) — and the daemon gives the
                    // supervisor no way to tell the two apart from the exit alone
                    // (same code, no line). `Wedged` is the safe default for
                    // both: a held lock clears on the caller's BOUNDED retry,
                    // while a persistent startup failure simply exhausts that
                    // retry budget and surfaces — whereas mislabelling a held
                    // lock as a hard failure would abort a recoverable start. (A
                    // precise split would need distinct daemon exit codes, a
                    // jeliyad change outside this crate.) Drop our stdin and read
                    // the child's exit (bounded); a non-zero exit is `Wedged`. A
                    // zero
                    // exit or a still-running child (force-killed) keeps the
                    // original handshake error.
                    drop(stdin);
                    // Capture the pgid (== leader pid) BEFORE the reaping wait:
                    // once `wait()` reaps the leader, `child.id()` is None and the
                    // isolated group can no longer be reached by pid. A faulty or
                    // overridden binary may have spawned a descendant into this
                    // group with its stdio closed and then exited; the reaped-leader
                    // arms must sweep the group so no descendant keeps holding the
                    // data-dir lock (or writing state) while the caller retries. The
                    // `_` arm leaves the child un-reaped, so `abandon_child` →
                    // `force_kill_tree` already reaches the group via `child.id()`.
                    let leader_pgid = child.id();
                    match tokio::time::timeout(self.timeouts.spawn, child.wait()).await {
                        Ok(Ok(status)) if !status.success() => {
                            if let Some(pgid) = leader_pgid {
                                // Propagate a verified group-cleanup failure: a
                                // descendant surviving SIGKILL (still holding the
                                // data-dir lock) must not be masked by `Wedged`/the
                                // spawn error, which invite a retry that would wedge.
                                process::kill_reaped_process_group(pgid).await?;
                            }
                            return Err(SupervisorError::Wedged);
                        }
                        Ok(Ok(_)) => {
                            if let Some(pgid) = leader_pgid {
                                // Propagate a verified group-cleanup failure: a
                                // descendant surviving SIGKILL (still holding the
                                // data-dir lock) must not be masked by `Wedged`/the
                                // spawn error, which invite a retry that would wedge.
                                process::kill_reaped_process_group(pgid).await?;
                            }
                            return Err(e);
                        }
                        _ => return Err(abandon_child(&mut child, e).await),
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
                    Err(e) => return Err(abandon_child(&mut child, e).await),
                };

            match announced {
                ReadyLine::Ready { pid, port } => {
                    // Our own fresh daemon announced itself. `finish_owned`'s
                    // served-generation gate governs it — including the deferred
                    // race where a proven-live incompatible incumbent exited before
                    // this spawn: the fresh daemon serves this build's generation,
                    // so validation adopts it (§6.7 case 13) or fails closed on a
                    // real drift, never on the vanished incumbent's stale portfile.
                    self.finish_owned(child, stdin, portfile, pid, port).await
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
                    let leader_pgid = child.id();
                    match tokio::time::timeout(self.timeouts.spawn, child.wait()).await {
                        Ok(Ok(status)) if status.success() => {
                            // Our probe child bowed out cleanly; we now fall through
                            // to ADOPT the incumbent. Confirm the isolated group is
                            // gone FIRST — a leaked descendant could still hold the
                            // data-dir lock and wreck the adopt — propagating a
                            // bounded cleanup failure instead of adopting over it.
                            if let Some(pgid) = leader_pgid {
                                process::kill_reaped_process_group(pgid).await?;
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
                                &mut child,
                                SupervisorError::Handshake(format!(
                                    "adopted-path child never exited: {e}"
                                )),
                            )
                            .await)
                        }
                        Err(_elapsed) => {
                            return Err(abandon_child(&mut child, SupervisorError::Wedged).await)
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
        mut child: Child,
        stdin: Option<ChildStdin>,
        portfile: Portfile,
        ready_pid: u32,
        ready_port: u16,
    ) -> Result<Sidecar, SupervisorError> {
        // The announced PID must be OUR spawned child's PID. A faulty or
        // overridden binary could print a `ready` line quoting the PID of an
        // existing compatible healthy daemon (matching its portfile) while being
        // a different process; binding to `child.id()` refuses that impersonation.
        let child_pid = child.id();
        if child_pid != Some(ready_pid) {
            return Err(abandon_child(
                &mut child,
                SupervisorError::Handshake(format!(
                    "ready line announced pid {ready_pid}, but the spawned child is pid {child_pid:?} — the binary announced a PID that is not itself"
                )),
            )
            .await);
        }
        if portfile.pid != ready_pid || portfile.port != ready_port {
            return Err(abandon_child(
                &mut child,
                SupervisorError::Handshake(format!(
                    "ready line says pid {ready_pid} port {ready_port} but the portfile says pid {} port {}",
                    portfile.pid, portfile.port
                )),
            )
            .await);
        }
        // Full agreement gate (loopback, declared+served generation, health/PID).
        match validate::validate_portfile(
            &self.data_dir,
            self.expected,
            self.strict_portfile_perms,
            &self.timeouts,
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
                    return Err(abandon_child(&mut child, mismatch).await);
                }
                Ok(self.owned_sidecar(child, stdin, validated.portfile))
            }
            Err(e) => {
                // A daemon WE spawned failed validation — the bundled binary
                // drifted from `expected` (R6). Stop it (we own it) and surface
                // the error rather than leaving a mispaired daemon running.
                Err(abandon_child(&mut child, e).await)
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
        match validate::validate_portfile(
            &self.data_dir,
            self.expected,
            self.strict_portfile_perms,
            &self.timeouts,
        )
        .await
        {
            Ok(validated) => Ok(self.adopted_sidecar(validated.portfile).await),
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
        let validated = validate::validate_portfile(
            &self.data_dir,
            self.expected,
            self.strict_portfile_perms,
            &self.timeouts,
        )
        .await?;
        Ok(self.adopted_sidecar(validated.portfile).await)
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

    async fn adopted_sidecar(&self, portfile: Portfile) -> Sidecar {
        // Capture a handle to the `daemon.lock` inode the daemon holds RIGHT NOW, at
        // adoption — the moment it was proven alive — and retain it so `stop_adopted`
        // proves the daemon's exit against THIS inode, not one re-opened at shutdown
        // (which a cleanup tool could have unlinked/replaced with an unrelated
        // process's lock). BOUNDED and OFF the executor (an `open` on a stalled mount
        // blocks); a miss yields `None`, which `stop_adopted` fails closed on.
        let deadline = validate::deadline_from(self.timeouts.teardown);
        let adopted_lock = validate::snapshot_held_lock(&self.data_dir, deadline).await;
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
        let mut child = {
            let mut attempts = 0u32;
            loop {
                let remaining =
                    spawn_deadline.saturating_duration_since(tokio::time::Instant::now());
                // The spawn runs on a blocking thread and HANDS its outcome back over
                // a oneshot. The blocking worker OWNS cleanup on the timeout path: if
                // we time out and drop the receiver, its `send` fails and it SIGKILLs
                // the late child's whole group itself, synchronously, on its own
                // blocking thread — which a runtime shutdown waits for — so a late
                // exec (dropped with `kill_on_drop(false)`) cannot leak its group even
                // if the caller drops the runtime. A detached async cleanup task would
                // instead be cancelled on that drop. The oneshot makes the hand-off
                // race-free: exactly one of {we receive the child, the worker cleans
                // it up} happens (`timeout` polls the receiver before its timer, so a
                // send that lands at the deadline is still delivered, not abandoned).
                let (tx, rx) = tokio::sync::oneshot::channel::<(Command, std::io::Result<Child>)>();
                tokio::task::spawn_blocking(move || {
                    let result = cmd.spawn();
                    // If the receiver is gone (the caller timed out) AND the spawn
                    // produced a live child, reclaim it and tear down its group so it
                    // cannot survive. A send that succeeds, or a returned spawn ERROR
                    // (no child), needs nothing.
                    if let Err((_cmd, Ok(late))) = tx.send((cmd, result)) {
                        process::force_kill_group_blocking(late);
                    }
                });
                let (returned, result) = match tokio::time::timeout(remaining, rx).await {
                    Ok(Ok(pair)) => pair,
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
                    Ok(c) => break c,
                    Err(e) if e.raw_os_error() == Some(26) && attempts < 10 => {
                        attempts += 1;
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    Err(e) => return Err(SupervisorError::Spawn(e)),
                }
            }
        };
        let stdin = child.stdin.take();

        if let Some(mut stderr) = child.stderr.take() {
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

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SupervisorError::Handshake("no stdout pipe".to_owned()))?;
        Ok((
            child,
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
    Child,
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

/// Read the first stdout line that starts with `{` (skipping any stray
/// human-readable output), parse it as a [`ReadyLine`], all within `budget`.
async fn read_announcement(
    lines: &mut Lines<BufReader<Take<ChildStdout>>>,
    budget: Duration,
) -> Result<ReadyLine, SupervisorError> {
    let line = tokio::time::timeout(budget, async {
        loop {
            match lines.next_line().await {
                Ok(Some(line)) if line.trim_start().starts_with('{') => return Ok(line),
                // Skip a non-JSON line (belt-and-suspenders; the daemon emits the
                // JSON first) and keep reading.
                Ok(Some(_)) => continue,
                Ok(None) => {
                    return Err(SupervisorError::Handshake(
                        "stdout closed before the announcement".to_owned(),
                    ))
                }
                Err(e) => {
                    return Err(SupervisorError::Handshake(format!(
                        "could not read stdout: {e}"
                    )))
                }
            }
        }
    })
    .await
    .map_err(|_| SupervisorError::Handshake(format!("no announcement within {budget:?}")))??;

    serde_json::from_str::<ReadyLine>(&line).map_err(|e| {
        // Do NOT echo the raw line OR the serde error's Display: both reproduce
        // unvalidated, child-controlled content up to MAX_ANNOUNCEMENT_BYTES that a
        // corrupted/overridden daemon could stuff with an `auth_token` or other
        // sensitive value (serde's Display echoes a mis-typed field VALUE, e.g.
        // `invalid type: string "…"`). Callers log or display a Handshake error,
        // so either would bypass the crate's token-redaction boundary (spec §7.2).
        // Report only the CONTENT-FREE classification and position plus the
        // withheld byte count.
        SupervisorError::Handshake(format!(
            "the daemon's announcement was not valid JSON ({:?} at line {} column {}); {} bytes of raw output withheld (unvalidated child content)",
            e.classify(),
            e.line(),
            e.column(),
            line.len()
        ))
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
}
