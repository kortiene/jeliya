//! Sidecar lifecycle: the process-supervision contract from
//! `docs/PROTOCOL.md` — single-instance lock per data dir, the `daemon.json`
//! portfile, adoption of an already-running daemon, tracing setup with a
//! rolling log file, and graceful shutdown.
//!
//! The contract exists so a parent process (the desktop app, a script, a
//! service manager) can own the daemon without guessing: spawn `jeliyad
//! --supervised`, read one JSON line from stdout (`ready` or
//! `already_running`), and find everything else — including the WS auth
//! token — in the portfile.

use std::fs::{self, File, OpenOptions};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{info, warn};

/// The portfile: the machine-readable identity of a running daemon, written
/// atomically into the data dir after bind and removed on graceful shutdown.
/// Contains the auth token, so it is created user-readable only (0600) on
/// Unix; on Windows the per-user data dir ACLs carry the same intent.
pub const PORTFILE_NAME: &str = "daemon.json";
/// Advisory lock held for the daemon's whole life. The OS releases it on any
/// process death (including `kill -9`), so a stale lock cannot outlive a
/// crash the way a stale portfile can.
pub const LOCKFILE_NAME: &str = "daemon.lock";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Portfile {
    pub schema: u32,
    pub pid: u32,
    pub port: u16,
    pub http: String,
    pub ws: String,
    pub version: String,
    pub protocol: u32,
    pub data_dir: String,
    pub auth_token: String,
    pub started_at_ms: u64,
}

pub fn portfile_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PORTFILE_NAME)
}

/// Open (creating if needed) the lockfile the instance lock is taken on.
pub fn lock_file(data_dir: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join(LOCKFILE_NAME))
}

/// A fresh 256-bit WS auth token, hex-encoded. Generated per daemon start and
/// published only through the 0600 portfile and the browser-only
/// `/api/session` endpoint.
pub fn generate_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("OS CSPRNG unavailable: {e}"))?;
    Ok(hex::encode(bytes))
}

/// Write the portfile atomically (temp file + rename) so a concurrently
/// reading supervisor never sees a torn write, with 0600 permissions on Unix.
pub fn write_portfile(data_dir: &Path, portfile: &Portfile) -> std::io::Result<PathBuf> {
    let path = portfile_path(data_dir);
    let tmp = data_dir.join(format!("{PORTFILE_NAME}.tmp"));
    let body = serde_json::to_string_pretty(portfile).expect("portfile serializes");
    {
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp)?;
        use std::io::Write;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, &path)?;
    Ok(path)
}

pub fn read_portfile(data_dir: &Path) -> Option<Portfile> {
    let raw = fs::read_to_string(portfile_path(data_dir)).ok()?;
    let portfile: Portfile = serde_json::from_str(&raw).ok()?;
    (portfile.schema == 1).then_some(portfile)
}

pub fn remove_portfile(data_dir: &Path) {
    let path = portfile_path(data_dir);
    if let Err(err) = fs::remove_file(&path) {
        if err.kind() != std::io::ErrorKind::NotFound {
            warn!("could not remove the portfile {}: {err}", path.display());
        }
    }
}

/// Acquire the single-instance lock, held for the daemon's whole life on
/// success. If a *healthy* daemon already owns this data dir, report it
/// machine-readably (`already_running`, exit 0) so a supervisor adopts it
/// instead of duplicating.
///
/// The key subtlety is the canonical restart: a supervisor SIGTERMs the old
/// daemon and immediately respawns. The old daemon still holds the lock while
/// its bounded graceful shutdown runs, so we must not give up on the first
/// failed `try_write` — we re-attempt the lock every iteration and succeed the
/// instant the incumbent's lock frees, continuing as a fresh start. Only a
/// held lock with NO healthy daemon behind it and no progress within the window
/// is treated as wedged (exit 1).
pub async fn acquire_or_adopt<'a>(
    lock: &'a mut fd_lock::RwLock<File>,
    data_dir: &Path,
) -> fd_lock::RwLockWriteGuard<'a, File> {
    // Wait until the lock is free (adopting a healthy incumbent or exiting if
    // the data dir is wedged); this holds no escaping borrow of `lock`.
    wait_for_free_lock(lock, data_dir).await;
    if let Ok(guard) = lock.try_write() {
        return guard;
    }
    // Extremely rare lost race: a concurrent start grabbed the lock in the gap
    // since `wait_for_free_lock` saw it free. That starter now owns the data
    // dir — adopt it (or exit 1 for a supervisor respawn). This path borrows
    // only `data_dir`, never `lock` again, which is what lets the guard above
    // return cleanly (NLL cannot otherwise prove a retry loop drops the borrow).
    for _ in 0..=20 {
        if let Some(portfile) = read_portfile(data_dir) {
            if health_check(&portfile, data_dir).await {
                report_already_running(&portfile, data_dir);
                std::process::exit(0);
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    std::process::exit(1);
}

/// Block until this data dir's instance lock is acquirable, or terminate the
/// process (adopt a healthy incumbent → exit 0; wedged data dir → exit 1).
/// Returns without holding the lock — the caller acquires it.
async fn wait_for_free_lock(lock: &mut fd_lock::RwLock<File>, data_dir: &Path) {
    // Up to ~60 * 250ms ≈ 15s of tolerance for an incumbent's teardown (its own
    // room-close timeout is bounded at 10s), beyond each health probe's cost.
    for attempt in 0..=60 {
        if lock.try_write().is_ok() {
            return;
        }
        // Someone holds the lock. A healthy incumbent means adopt-and-exit; an
        // unhealthy one (mid-shutdown, or mid-start before it bound) means wait
        // for the lock to free and retry.
        if let Some(portfile) = read_portfile(data_dir) {
            if health_check(&portfile, data_dir).await {
                report_already_running(&portfile, data_dir);
                std::process::exit(0);
            }
        }
        if attempt == 60 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    eprintln!(
        "error: another process holds the jeliyad lock for {} but no healthy daemon \
         answered on its advertised port; if a daemon is still starting or just \
         crashed, retry in a moment",
        data_dir.display()
    );
    std::process::exit(1);
}

fn report_already_running(portfile: &Portfile, data_dir: &Path) {
    println!(
        "{}",
        json!({
            "event": "already_running",
            "pid": portfile.pid,
            "port": portfile.port,
            "http": portfile.http,
            "ws": portfile.ws,
            "version": portfile.version,
            "protocol": portfile.protocol,
            "data_dir": portfile.data_dir,
            "portfile": portfile_path(data_dir).display().to_string(),
        })
    );
    println!(
        "jeliyad is already running for this data dir (pid {}, {}); \
         connect to it instead of starting a second one.",
        portfile.pid, portfile.http
    );
}

/// `GET /api/health` on the advertised port and require the answering daemon
/// to be the portfile's process (pid match) on the same data dir — a port
/// number recycled by an unrelated process must not be adopted.
async fn health_check(portfile: &Portfile, expect_data_dir: &Path) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, portfile.port));
    let connect = tokio::time::timeout(Duration::from_millis(500), TcpStream::connect(addr));
    let Ok(Ok(mut stream)) = connect.await else {
        return false;
    };
    let request = format!(
        "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        portfile.port
    );
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut response = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut response));
    if !matches!(read.await, Ok(Ok(_))) {
        return false;
    }
    let text = String::from_utf8_lossy(&response);
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return false;
    }
    let Ok(health) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
        return false;
    };
    health.get("pid").and_then(serde_json::Value::as_u64) == Some(u64::from(portfile.pid))
        && health
            .get("data_dir")
            .and_then(serde_json::Value::as_str)
            .map(|dir| Path::new(dir) == expect_data_dir)
            .unwrap_or(false)
}

/// Tracing to stderr plus a daily-rolling file in `<data_dir>/logs`, filtered
/// by `JELIYAD_LOG` (falling back to `RUST_LOG`, then `info`). Returns the
/// appender guard that must stay alive for the daemon's lifetime.
pub fn init_tracing(data_dir: &Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    let filter = EnvFilter::try_from_env("JELIYAD_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr_layer = fmt::layer().with_writer(std::io::stderr).with_target(false);

    let logs_dir = data_dir.join("logs");
    match fs::create_dir_all(&logs_dir) {
        Ok(()) => {
            let appender = tracing_appender::rolling::daily(&logs_dir, "jeliyad.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let file_layer = fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_target(false);
            tracing_subscriber::registry()
                .with(filter)
                .with(stderr_layer)
                .with(file_layer)
                .init();
            Some(guard)
        }
        Err(err) => {
            tracing_subscriber::registry()
                .with(filter)
                .with(stderr_layer)
                .init();
            warn!(
                "could not create {}: {err}; file logging disabled",
                logs_dir.display()
            );
            None
        }
    }
}

/// Close every open room (releasing its blob locks and network session) and
/// remove the portfile. Bounded: a room whose teardown hangs must not turn
/// SIGTERM into a zombie, so after 10s we exit anyway and note it.
pub async fn graceful_shutdown(state: &crate::AppState, reason: &str) {
    info!("shutting down ({reason})");
    let close_all = async {
        for room_id in state.supervisor.open_rooms() {
            match state.supervisor.close_room(&room_id).await {
                Ok(()) => info!("closed room {room_id}"),
                Err(err) => warn!("could not close room {room_id} cleanly: {err}"),
            }
        }
    };
    if tokio::time::timeout(Duration::from_secs(10), close_all)
        .await
        .is_err()
    {
        warn!("room teardown did not finish within 10s; exiting anyway");
    }
    remove_portfile(&state.data_dir);
    info!("shutdown complete");
}
