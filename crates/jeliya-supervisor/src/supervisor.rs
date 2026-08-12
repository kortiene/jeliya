//! The supervisor entry point: resolve the binary and data dir, spawn-or-adopt
//! a supervised daemon, validate ready↔portfile↔health agreement, and (opt-in)
//! evict a proven-owned incompatible incumbent.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
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
        // the daemon canonicalizes too.
        let data_dir = data_dir.canonicalize().unwrap_or(data_dir);

        let (binary, binary_tried) = match config.binary {
            Some(explicit) if explicit.is_file() => (Some(explicit), Vec::new()),
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
        self.start_or_adopt_inner(self.replace_incompatible).await
    }

    fn start_or_adopt_inner<'a>(
        &'a self,
        allow_evict: bool,
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

            let (mut child, stdin, mut lines) = self.spawn(&binary)?;
            let announced = match read_announcement(&mut lines, self.timeouts.spawn).await {
                Ok(line) => line,
                Err(e) => {
                    // We own this child; do not leak it on a handshake failure.
                    let _ = process::force_kill_tree(&mut child).await;
                    return Err(e);
                }
            };

            // The portfile is written before the announcement, so it is readable
            // the instant the line parses.
            let portfile = match portfile::read_portfile(&self.data_dir, self.strict_portfile_perms)
            {
                Ok(pf) => pf,
                Err(e) => {
                    let _ = process::force_kill_tree(&mut child).await;
                    return Err(e);
                }
            };

            match announced {
                ReadyLine::Ready { pid, port } => {
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
                    match tokio::time::timeout(self.timeouts.spawn, child.wait()).await {
                        Ok(Ok(status)) if status.success() => {}
                        Ok(Ok(_status)) => return Err(SupervisorError::Wedged),
                        Ok(Err(e)) => {
                            let _ = process::force_kill_tree(&mut child).await;
                            return Err(SupervisorError::Handshake(format!(
                                "adopted-path child never exited: {e}"
                            )));
                        }
                        Err(_elapsed) => {
                            let _ = process::force_kill_tree(&mut child).await;
                            return Err(SupervisorError::Wedged);
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
        if portfile.pid != ready_pid || portfile.port != ready_port {
            let _ = process::force_kill_tree(&mut child).await;
            return Err(SupervisorError::Handshake(format!(
                "ready line says pid {ready_pid} port {ready_port} but the portfile says pid {} port {}",
                portfile.pid, portfile.port
            )));
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
                    let _ = process::force_kill_tree(&mut child).await;
                    return Err(mismatch);
                }
                Ok(self.owned_sidecar(child, stdin, validated.portfile))
            }
            Err(e) => {
                // A daemon WE spawned failed validation — the bundled binary
                // drifted from `expected` (R6). Stop it (we own it) and surface
                // the error rather than leaving a mispaired daemon running.
                let _ = process::force_kill_tree(&mut child).await;
                Err(e)
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
            Ok(validated) => Ok(self.adopted_sidecar(validated.portfile)),
            Err(SupervisorError::GenerationMismatch { expected, actual }) if allow_evict => {
                // Skew path (spec §6.7): replace ONLY a proven-owned incumbent.
                // Re-prove ownership independently (the validation short-circuited
                // on the declared mismatch before its own health step), so a
                // recycled/dead PID or foreign process is never signalled.
                if validate::prove_owned(&portfile, &self.timeouts).await {
                    process::sigterm_foreign(portfile.pid)?;
                    if validate::wait_health_dark(
                        portfile.pid,
                        portfile.port,
                        self.timeouts.evict,
                        &self.timeouts,
                    )
                    .await
                    {
                        // Respawn exactly once (no further eviction), against the
                        // now-free data dir.
                        self.start_or_adopt_inner(false).await
                    } else {
                        Err(SupervisorError::ShutdownTimedOut { pid: portfile.pid })
                    }
                } else {
                    // Unprovable incumbent (fault #14): fail closed, signal
                    // nothing.
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
        Ok(self.adopted_sidecar(validated.portfile))
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
        }
    }

    fn adopted_sidecar(&self, portfile: Portfile) -> Sidecar {
        Sidecar {
            portfile,
            ownership: Ownership::Adopted,
            data_dir: self.data_dir.clone(),
            expected: self.expected,
            strict_portfile_perms: self.strict_portfile_perms,
            timeouts: self.timeouts,
        }
    }

    /// Spawn `jeliyad --supervised --data-dir <dir> --port 0 [--loopback]` with
    /// stdin/stdout/stderr piped, `kill_on_drop(false)` (the stdin pipe, not
    /// `Drop`, is the parent-death mechanism), and the child in its own process
    /// group. Drains stderr forever on a background task — an unread full stderr
    /// pipe deadlocks the daemon's synchronous tracing layer (the same records
    /// survive in `<data_dir>/logs`).
    fn spawn(&self, binary: &Path) -> Result<SpawnedDaemon, SupervisorError> {
        std::fs::create_dir_all(&self.data_dir).map_err(|e| {
            SupervisorError::PortfileUnreadable {
                path: self.data_dir.clone(),
                why: format!("could not create the data dir: {e}"),
            }
        })?;

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

        let mut child = cmd.spawn().map_err(SupervisorError::Spawn)?;
        let stdin = child.stdin.take();

        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(_)) = lines.next_line().await {}
            });
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SupervisorError::Handshake("no stdout pipe".to_owned()))?;
        Ok((child, stdin, BufReader::new(stdout).lines()))
    }
}

/// The pieces [`Supervisor::spawn`] hands back: the child process, its held
/// stdin (the parent-death pipe, kept for the daemon's life), and a line reader
/// over its stdout for the announcement.
type SpawnedDaemon = (Child, Option<ChildStdin>, Lines<BufReader<ChildStdout>>);

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
    lines: &mut Lines<BufReader<ChildStdout>>,
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

    serde_json::from_str::<ReadyLine>(&line)
        .map_err(|e| SupervisorError::Handshake(format!("unparseable announcement {line:?}: {e}")))
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

    if let Some(explicit) = std::env::var_os("JELIYAD_BIN") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
        tried.push(format!("JELIYAD_BIN={}", path.display()));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join(bundled_name());
            if bundled.is_file() {
                return Ok(bundled);
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
        if repo.is_file() {
            return Ok(repo);
        }
        tried.push(repo.display().to_string());
    }
    #[cfg(not(debug_assertions))]
    tried.push("(the target/debug fallback is debug-only)".to_owned());

    Err(tried)
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
