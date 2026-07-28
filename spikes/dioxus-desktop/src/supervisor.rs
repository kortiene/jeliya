//! The ownership half of issue #159: spawn `jeliyad`, or adopt one that is
//! already serving this data dir, and tear down correctly in each case.
//!
//! The contract is documented in `crates/jeliyad/src/lifecycle.rs` and was
//! confirmed against a real `jeliyad 0.6.1` before this was written:
//!
//! - the daemon writes exactly one JSON line to stdout, `ready` or
//!   `already_running`, and `already_running` is followed by `exit 0`;
//! - the portfile `daemon.json` is written 0600 and carries the auth token;
//! - `--supervised` makes the daemon exit when its stdin reaches EOF, and a
//!   graceful exit removes the portfile;
//! - a second daemon on a live data dir does NOT disturb the first.
//!
//! The last point is the whole reason this module distinguishes [`Sidecar`]
//! variants. A supervisor that kills "the daemon" on shutdown without knowing
//! whether it started it will kill someone else's.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};

/// The daemon's portfile. Field-for-field the struct `jeliyad` writes; the
/// spike re-declares it rather than importing `jeliyad`, because the whole
/// point is to prove an independent client can speak this contract.
#[derive(Debug, Clone, Deserialize)]
pub struct Portfile {
    pub schema: u32,
    pub pid: u32,
    pub port: u16,
    pub http: String,
    pub ws: String,
    pub version: String,
    pub protocol: u32,
    pub data_dir: String,
    /// Never rendered, never logged, never handed to the WebView. See
    /// [`Sidecar::token`].
    auth_token: String,
    pub started_at_ms: u64,
}

/// The daemon's first stdout line.
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum ReadyLine {
    Ready { port: u16, pid: u32 },
    AlreadyRunning { port: u16, pid: u32 },
}

#[derive(Debug)]
pub enum Error {
    NoBinary(String),
    Spawn(std::io::Error),
    /// The daemon exited or said something unparseable instead of announcing.
    Handshake(String),
    Portfile(String),
    /// The data dir is wedged: a held lock with no healthy daemon behind it.
    Wedged,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBinary(tried) => write!(f, "no jeliyad binary found; tried {tried}"),
            Self::Spawn(e) => write!(f, "could not spawn jeliyad: {e}"),
            Self::Handshake(why) => write!(f, "jeliyad did not announce itself: {why}"),
            Self::Portfile(why) => write!(f, "could not read the portfile: {why}"),
            Self::Wedged => write!(
                f,
                "the data dir is locked but no healthy daemon answered; retry in a moment"
            ),
        }
    }
}

/// How this process came to have a daemon — and therefore what it owes that
/// daemon at shutdown.
#[derive(Debug)]
enum Ownership {
    /// We started it. Closing its stdin is our shutdown signal.
    Owned {
        child: Child,
        /// Held open for the daemon's whole life. Dropping this IS the
        /// parent-death signal, so it must not be dropped early — which is
        /// exactly the bug that made a `< /dev/null` spawn exit instantly.
        stdin: Option<ChildStdin>,
    },
    /// It was already serving this data dir. We are a guest; we do not get to
    /// end its life.
    Adopted,
}

/// A live daemon this process can talk to.
#[derive(Debug)]
pub struct Sidecar {
    pub portfile: Portfile,
    ownership: Ownership,
}

impl Sidecar {
    /// True when this process started the daemon and may therefore stop it.
    #[must_use]
    pub fn is_owned(&self) -> bool {
        matches!(self.ownership, Ownership::Owned { .. })
    }

    /// The WebSocket URL, without the token. Safe to display.
    #[must_use]
    pub fn ws_url(&self) -> &str {
        &self.portfile.ws
    }

    /// The auth token, deliberately behind a method rather than a public field
    /// so every read is greppable. The spike's one rule about this value is
    /// that it reaches the native transport and nothing else — never a Dioxus
    /// prop, never a DOM attribute, never a log line.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.portfile.auth_token
    }

    /// Stop an owned daemon and wait for it; leave an adopted one alone.
    ///
    /// Returns whether this call actually stopped anything, so the caller can
    /// assert the adopted case really is a no-op.
    pub async fn shutdown(mut self) -> std::io::Result<bool> {
        match &mut self.ownership {
            Ownership::Adopted => Ok(false),
            Ownership::Owned { child, stdin } => {
                // Closing stdin is the documented `--supervised` signal, and it
                // is graceful: the daemon removes its portfile on the way out.
                // Measured at 0.00s against jeliyad 0.6.1, but the daemon's own
                // room-close path is bounded at 10s, so allow room for it.
                drop(stdin.take());
                match tokio::time::timeout(std::time::Duration::from_secs(15), child.wait()).await {
                    Ok(status) => {
                        status?;
                    }
                    Err(_) => {
                        // The graceful path did not finish in time. A spike
                        // says so rather than pretending teardown was clean.
                        child.kill().await?;
                        child.wait().await?;
                    }
                }
                Ok(true)
            }
        }
    }
}

/// Resolve the daemon binary the way `app/README.md` documents for desktop:
/// the `JELIYAD_BIN` override wins, then a sidecar bundled next to this
/// executable, then the repo's debug build.
///
/// Step 4 of the documented order — an installed `jeliyad` on `PATH` — is
/// deliberately NOT implemented here. A packaged shell that silently falls
/// back to whatever version is on `PATH` can pair a new UI with an old daemon,
/// and this spike has no protocol-generation check to catch that.
pub fn resolve_jeliyad() -> Result<PathBuf, Error> {
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
            // The layout `build.sh` produces: the daemon sits beside the shell.
            let bundled = dir.join("jeliyad");
            if bundled.is_file() {
                return Ok(bundled);
            }
            tried.push(bundled.display().to_string());
        }
    }

    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("target/debug/jeliyad");
    if repo.is_file() {
        return Ok(repo);
    }
    tried.push(repo.display().to_string());

    Err(Error::NoBinary(tried.join(", ")))
}

/// Start a daemon for `data_dir`, or adopt the one already serving it.
///
/// Blocks until the daemon has announced itself, so a caller that gets `Ok`
/// has a daemon that is bound and answering — not one that is still starting.
pub async fn start_or_adopt(binary: &Path, data_dir: &Path) -> Result<Sidecar, Error> {
    std::fs::create_dir_all(data_dir)
        .map_err(|e| Error::Portfile(format!("could not create {}: {e}", data_dir.display())))?;

    let mut child = Command::new(binary)
        .arg("--supervised")
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--port")
        .arg("0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // DELIBERATELY false. `kill_on_drop(true)` would SIGKILL the daemon
        // whenever this struct is dropped, which sounds safer and is actually
        // misleading twice over:
        //
        // 1. It masks the mechanism under test. With it on, the parent-death
        //    test passes because Drop killed the child, not because closing
        //    stdin did — so a broken `--supervised` contract would still look
        //    green.
        // 2. It does not help in the case it appears to cover. If the shell is
        //    SIGKILLed or panics hard, Rust runs no destructors, so
        //    `kill_on_drop` never fires. What actually ends the daemon is the
        //    OS closing this process's fds, which EOFs the child's stdin.
        //
        // Leaving it false means the stdin pipe is the only thing standing
        // between a dead shell and an orphaned daemon — which is exactly the
        // production situation, so the tests measure the real guarantee.
        .kill_on_drop(false)
        .spawn()
        .map_err(Error::Spawn)?;

    let stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Handshake("no stdout pipe".into()))?;

    let mut lines = BufReader::new(stdout).lines();
    let line = tokio::time::timeout(std::time::Duration::from_secs(30), lines.next_line())
        .await
        .map_err(|_| Error::Handshake("no announcement within 30s".into()))?
        .map_err(|e| Error::Handshake(format!("could not read stdout: {e}")))?
        .ok_or_else(|| Error::Handshake("stdout closed before the announcement".into()))?;

    let announced: ReadyLine = serde_json::from_str(&line)
        .map_err(|e| Error::Handshake(format!("unparseable first line {line:?}: {e}")))?;

    // The portfile is what carries the token, and it is written before the
    // announcement, so it is readable the moment we have parsed the line.
    let portfile = read_portfile(data_dir)?;

    match announced {
        ReadyLine::Ready { port, pid } => {
            // The announcement and the portfile must describe the same daemon.
            // If they disagree, a stale portfile from a previous run is being
            // read and every subsequent connection would target the wrong port.
            if portfile.port != port || portfile.pid != pid {
                return Err(Error::Handshake(format!(
                    "ready line says pid {pid} port {port} but the portfile says pid {} port {}",
                    portfile.pid, portfile.port
                )));
            }
            Ok(Sidecar {
                portfile,
                ownership: Ownership::Owned {
                    child,
                    stdin,
                },
            })
        }
        ReadyLine::AlreadyRunning { port, pid } => {
            // The daemon we spawned has bowed out with exit 0; the incumbent is
            // the real one. Drop our stdin first so the exiting child is not
            // held open by it.
            drop(stdin);
            let status = child
                .wait()
                .await
                .map_err(|e| Error::Handshake(format!("adopted-path child never exited: {e}")))?;
            if !status.success() {
                return Err(Error::Wedged);
            }
            if portfile.port != port || portfile.pid != pid {
                return Err(Error::Handshake(format!(
                    "already_running says pid {pid} port {port} but the portfile says pid {} port {}",
                    portfile.pid, portfile.port
                )));
            }
            Ok(Sidecar {
                portfile,
                ownership: Ownership::Adopted,
            })
        }
    }
}

fn read_portfile(data_dir: &Path) -> Result<Portfile, Error> {
    let path = data_dir.join("daemon.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| Error::Portfile(format!("{}: {e}", path.display())))?;
    let portfile: Portfile = serde_json::from_str(&raw)
        .map_err(|e| Error::Portfile(format!("{} is not a portfile: {e}", path.display())))?;
    if portfile.schema != 1 {
        return Err(Error::Portfile(format!(
            "portfile schema {} is not the 1 this spike speaks",
            portfile.schema
        )));
    }
    Ok(portfile)
}
