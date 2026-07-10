//! `jeliyad` — the Jeliya daemon: a local-only WebSocket server at
//! `ws://127.0.0.1:<port>/ws` implementing `docs/PROTOCOL.md` over
//! `jeliya-core` (the sole consumer of the iroh-rooms SDK).
//!
//! Local-only by construction: the listener binds `127.0.0.1` and nothing
//! else — there is no flag to bind another interface, so the protocol's
//! "MUST refuse to bind non-loopback interfaces" holds trivially.
//!
//! Sidecar-ownable by contract (`docs/PROTOCOL.md`, "Process supervision"):
//! one instance per data dir (advisory lock; a second launch reports the
//! running daemon as `already_running` and exits), a machine-readable `ready`
//! JSON line on stdout plus a `daemon.json` portfile carrying the bound port
//! and the WS auth token, graceful teardown on SIGTERM/SIGINT, and
//! `--supervised` mode that exits when the parent closes stdin.

mod lifecycle;
mod rpc;
mod serve;

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

use jeliya_core::error::ErrorKind;
use jeliya_core::supervisor::RoomSupervisor;

/// Major version of the protocol spoken on `/ws` (docs/PROTOCOL.md). Part of
/// the supervision contract: an app adopts a running daemon only when this
/// matches what it was built against; on mismatch it must not spawn a second
/// daemon on the same data dir, but stop the old one and respawn.
pub(crate) const PROTOCOL_VERSION: u32 = 1;

/// The daemon tick for the reconcile safety net + peer-change drain (~300ms per
/// the protocol build notes). Since issue #83 live `room.event` pushes arrive
/// immediately via each room's `room_events` pump, so this tick is no longer the
/// latency path — only the reconcile that a lossy broadcast cannot let drift.
const PUSH_TICK: Duration = Duration::from_millis(300);

#[derive(Parser, Debug)]
#[command(
    name = "jeliyad",
    version,
    about = "The Jeliya daemon — private peer-to-peer rooms for humans and AI agents.\nServes the app at http://127.0.0.1:7420/ (change with --port)."
)]
struct Args {
    /// TCP port on 127.0.0.1 to serve `http://127.0.0.1:<port>/` and `/ws`.
    /// `0` asks the OS for any free port (read it back from the `ready` line
    /// or the portfile).
    #[arg(long, default_value_t = 7420)]
    port: u16,
    /// Data directory (identity, rooms.db, blobs, downloads, local state).
    /// Defaults to a per-user platform data directory so a GUI launch from an
    /// arbitrary working directory never scatters or duplicates identities.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Serve the web UI from this directory instead of any embedded assets
    /// (decouples UI iteration from a daemon rebuild).
    #[arg(long)]
    ui_dir: Option<PathBuf>,
    /// Do not open the web UI in a browser on startup.
    #[arg(long, default_value_t = false)]
    no_open: bool,
    /// Use the SDK's loopback/CI network mode instead of the real network.
    #[arg(long, default_value_t = false)]
    loopback: bool,
    /// Sidecar mode for a supervising parent process: shut down gracefully
    /// when stdin reaches EOF (the portable parent-death signal on all three
    /// OSes) and never auto-open a browser.
    #[arg(long, default_value_t = false)]
    supervised: bool,
}

/// Shared server state: the supervisor and the push fan-out channel.
///
/// The supervisor is shared as a plain `Arc` (no daemon-wide async mutex): its
/// own internal locks are held only for brief map operations, never across a
/// network `.await`, so one client's slow request can no longer freeze every
/// other client or the push loop.
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) supervisor: Arc<RoomSupervisor>,
    pub(crate) data_dir: PathBuf,
    pub(crate) push_tx: broadcast::Sender<String>,
    /// Per-start WS/API auth token (see `lifecycle::generate_token`).
    pub(crate) auth_token: Arc<String>,
    /// Graceful-shutdown trigger; the string is the human-readable reason.
    pub(crate) shutdown_tx: mpsc::Sender<String>,
    /// The actually bound port (`--port 0` resolves here).
    pub(crate) port: u16,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let data_dir = args.data_dir.clone().unwrap_or_else(default_data_dir);
    if let Err(err) = std::fs::create_dir_all(&data_dir) {
        eprintln!(
            "error: could not create the data dir {}: {err}",
            data_dir.display()
        );
        std::process::exit(1);
    }
    // Canonical form so lock/portfile identity checks compare like with like
    // regardless of how the path was spelled on the command line.
    let data_dir = data_dir.canonicalize().unwrap_or(data_dir);

    let log_guard = lifecycle::init_tracing(&data_dir);

    // Single instance per data dir, held for the daemon's whole life. If a
    // daemon already owns this data dir, `acquire_or_adopt` reports it and
    // exits 0 (a supervisor then adopts it) — two daemons on one data dir is a
    // state-corruption scenario (state.json is last-writer-wins). It rides out
    // the brief overlap of a SIGTERM-then-respawn restart. The OS releases the
    // lock on any process death, so a crash cannot leave a stale lock behind.
    let mut instance_lock = match lifecycle::lock_file(&data_dir) {
        Ok(file) => fd_lock::RwLock::new(file),
        Err(err) => {
            error!("could not open the instance lockfile: {err}");
            std::process::exit(1);
        }
    };
    let _instance_guard = lifecycle::acquire_or_adopt(&mut instance_lock, &data_dir).await;

    let supervisor = match RoomSupervisor::new(data_dir.clone(), args.loopback) {
        Ok(sup) => sup,
        Err(err) => {
            error!("could not initialize the data dir: {err}");
            std::process::exit(1);
        }
    };

    let auth_token = match lifecycle::generate_token() {
        Ok(token) => token,
        Err(err) => {
            error!("could not generate the auth token: {err}");
            std::process::exit(1);
        }
    };

    // Where the web UI is served from: an explicit --ui-dir wins; otherwise the
    // assets embedded at build time (present only in an `embed-ui` build).
    let ui = serve::UiSource::resolve(args.ui_dir.clone());

    // Bind loopback ONLY (see the module doc). On a collision on an explicit
    // port, scan a small range upward rather than dying: a second launch (on a
    // *different* data dir; same-dir duplicates exit above) or an unrelated
    // occupant must not hard-fail a GUI start. `--port 0` binds exactly once
    // and lets the OS choose.
    let (listener, addr) = match bind_loopback(args.port, 20).await {
        Some(bound) => bound,
        None => {
            error!(
                "no free loopback port in {}..={}",
                args.port,
                args.port.saturating_add(20)
            );
            std::process::exit(1);
        }
    };
    if args.port != 0 && addr.port() != args.port {
        info!(
            "port {} was in use; bound {} instead",
            args.port,
            addr.port()
        );
    }

    let (push_tx, _) = broadcast::channel(1024);
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<String>(4);
    let state = AppState {
        supervisor: Arc::new(supervisor),
        data_dir: data_dir.clone(),
        push_tx,
        auth_token: Arc::new(auth_token.clone()),
        shutdown_tx: shutdown_tx.clone(),
        port: addr.port(),
    };

    let portfile = lifecycle::Portfile {
        schema: 1,
        pid: std::process::id(),
        port: addr.port(),
        http: format!("http://{addr}/"),
        ws: format!("ws://{addr}/ws"),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol: PROTOCOL_VERSION,
        data_dir: data_dir.display().to_string(),
        auth_token,
        started_at_ms: jeliya_core::now_ms(),
    };
    let portfile_path = match lifecycle::write_portfile(&data_dir, &portfile) {
        Ok(path) => path,
        Err(err) => {
            error!("could not write the portfile: {err}");
            std::process::exit(1);
        }
    };

    // The supervision contract: exactly one machine-readable JSON line on
    // stdout, first, before any human-readable output. The token is NOT here —
    // it lives in the 0600 portfile.
    println!(
        "{}",
        json!({
            "event": "ready",
            "pid": portfile.pid,
            "port": portfile.port,
            "http": portfile.http,
            "ws": portfile.ws,
            "version": portfile.version,
            "protocol": portfile.protocol,
            "data_dir": portfile.data_dir,
            "portfile": portfile_path.display().to_string(),
        })
    );
    if ui.is_serving() {
        println!(
            "jeliyad on {}  ({})  data dir: {}",
            portfile.http,
            portfile.ws,
            data_dir.display()
        );
    } else {
        println!(
            "jeliyad listening on {} (data dir: {})",
            portfile.ws,
            data_dir.display()
        );
    }

    tokio::spawn(push_loop(state.clone()));

    // Shutdown triggers. Ctrl-C covers all three OSes; SIGTERM is the Unix
    // service-manager/supervisor signal; stdin EOF is the portable
    // parent-death signal in --supervised mode (the parent holds our stdin
    // pipe; when it dies — even by kill -9 — the pipe closes).
    {
        let tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                let _ = tx.send("interrupt (ctrl-c)".to_owned()).await;
            }
        });
    }
    #[cfg(unix)]
    {
        let tx = shutdown_tx.clone();
        tokio::spawn(async move {
            let mut sigterm =
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(sig) => sig,
                    Err(err) => {
                        warn!("could not install the SIGTERM handler: {err}");
                        return;
                    }
                };
            if sigterm.recv().await.is_some() {
                let _ = tx.send("SIGTERM".to_owned()).await;
            }
        });
    }
    if args.supervised {
        let tx = shutdown_tx.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut stdin = tokio::io::stdin();
            let mut buf = [0u8; 256];
            loop {
                match stdin.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            let _ = tx.send("stdin closed (parent exited)".to_owned()).await;
        });
    }

    // Open the UI once we're bound and actually serving it (best-effort, never
    // fatal). Scripts, headless runs, and supervised sidecar runs never pop a
    // browser — in sidecar mode the parent app owns all UX.
    if ui.is_serving() && !args.no_open && !args.supervised {
        let url = format!("http://{addr}/");
        if let Err(err) = webbrowser::open(&url) {
            info!("could not open a browser ({err}); open {url} yourself");
        }
    }

    let reason = loop {
        tokio::select! {
            reason = shutdown_rx.recv() => {
                break reason.unwrap_or_else(|| "shutdown channel closed".to_owned());
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, _peer)) => {
                    let state = state.clone();
                    let ui = ui.clone();
                    tokio::spawn(async move {
                        serve::handle_conn(stream, state, ui).await;
                    });
                }
                Err(err) => {
                    warn!("accept failed: {err}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    };

    // Stop accepting BEFORE teardown: dropping the listener closes the port so a
    // concurrent restart's health probe fails fast (connection refused) instead
    // of blocking on a socket whose accept loop has stopped.
    drop(listener);
    lifecycle::graceful_shutdown(&state, &reason).await;
    // Flush the non-blocking file-log worker before the process dies —
    // std::process::exit runs no destructors, so drop the guard explicitly or
    // the shutdown records race the writer and are lost.
    drop(log_guard);
    std::process::exit(0);
}

/// The default per-user data directory (`~/Library/Application Support/Jeliya`,
/// `$XDG_DATA_HOME/Jeliya`, or `%APPDATA%\Jeliya`), falling back to a
/// cwd-relative dir only when no platform path is discoverable.
fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|dir| dir.join("Jeliya"))
        .unwrap_or_else(|| PathBuf::from("./.jeliya-data"))
}

/// Bind `127.0.0.1:port`, scanning up to `tries` ports upward past a collision
/// on an explicit port. `--port 0` binds exactly once (the OS picks). The
/// returned address is the listener's own `local_addr()` — the only truthful
/// answer once the OS has chosen a port.
async fn bind_loopback(port: u16, tries: u16) -> Option<(TcpListener, SocketAddr)> {
    let last = if port == 0 {
        port
    } else {
        port.saturating_add(tries)
    };
    for candidate in port..=last {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, candidate));
        match TcpListener::bind(addr).await {
            Ok(listener) => match listener.local_addr() {
                Ok(local) => return Some((listener, local)),
                Err(err) => {
                    error!("could not read the bound address for {addr}: {err}");
                    return None;
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(err) => {
                error!("could not bind {addr}: {err}");
                return None;
            }
        }
    }
    None
}

/// Drive the room-event push fan-out (issue #83).
///
/// Each open room gets a dedicated pump task that awaits its node's
/// `room_events` broadcast and pushes each new validated event as `room.event`
/// the moment it commits (sub-second latency, no hot tail poll). This ticker
/// (~300ms) supervises those pumps, runs the reconcile safety net
/// (`poll_new_events`, which a lossy broadcast cannot let drift and which keeps
/// the join-bootstrap window tied to live state), and drains each session's
/// `conn_events` broadcast to push `peers.changed` with truthful direct/relay
/// path info on any transition. The pump and the reconcile share the
/// supervisor's per-room `seen` set, so every event is pushed exactly once.
async fn push_loop(state: AppState) {
    let mut ticker = tokio::time::interval(PUSH_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Rooms with a live per-room `room_events` pump task. Shared with the pumps
    // so a pump deregisters itself on exit (room.close), letting a later re-open
    // re-spawn a fresh pump.
    let pumped: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    loop {
        ticker.tick().await;
        let sup = &state.supervisor;
        for room_id in sup.open_room_ids() {
            let room_str = room_id.to_string();

            // Ensure a live push pump for this room.
            let fresh = pumped
                .lock()
                .expect("pumped mutex poisoned")
                .insert(room_str.clone());
            if fresh {
                let state = state.clone();
                let pumped = pumped.clone();
                let key = room_str.clone();
                tokio::spawn(async move {
                    loop {
                        match state.supervisor.recv_room_events(&room_id).await {
                            Ok(events) => {
                                for event in events {
                                    let frame = json!({
                                        "push": "room.event",
                                        "data": { "room_id": key, "event": event },
                                    });
                                    let _ = state.push_tx.send(frame.to_string());
                                }
                            }
                            // The room closed: stop pumping and deregister so a
                            // later re-open re-spawns a fresh pump.
                            Err(err) if err.kind == ErrorKind::RoomNotOpen => break,
                            // A transient read error: the reconcile poll still
                            // covers pushes; back off briefly, then keep pumping.
                            Err(err) => {
                                warn!("room-event pump error for {key}: {err}");
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                        }
                    }
                    pumped.lock().expect("pumped mutex poisoned").remove(&key);
                });
            }

            // Reconcile safety net: re-scan the tail so a lagged/dropped
            // broadcast event is still pushed exactly once (shared `seen`).
            match sup.poll_new_events(&room_id).await {
                Ok(events) => {
                    for event in events {
                        let frame = json!({
                            "push": "room.event",
                            "data": { "room_id": room_str, "event": event },
                        });
                        let _ = state.push_tx.send(frame.to_string());
                    }
                }
                Err(err) => warn!("push reconcile failed for {room_str}: {err}"),
            }
            if sup.drain_conn_changes(&room_id) {
                if let Ok(peers) = sup.peers_status(&room_str).await {
                    let frame = json!({
                        "push": "peers.changed",
                        "data": { "room_id": room_str, "peers": peers },
                    });
                    let _ = state.push_tx.send(frame.to_string());
                }
            }
        }
    }
}

/// Whether a bare `host[:port]` (the `Host` header shape) denotes loopback.
/// The Host gate on `/ws` and `/api/*` blocks DNS-rebinding: a hostile page
/// can point its own domain at 127.0.0.1 and fetch same-origin, but its
/// requests still carry `Host: evil.example`.
pub(crate) fn host_header_is_loopback(hostport: &str) -> bool {
    let host = if let Some(bracketed) = hostport.strip_prefix('[') {
        // `[ipv6]` or `[ipv6]:port`
        bracketed.split_once(']').map_or(bracketed, |(h, _)| h)
    } else {
        hostport.split_once(':').map_or(hostport, |(h, _)| h)
    };
    // Exact loopback only: `localhost`, or an IP literal in 127.0.0.0/8 or ::1.
    // A domain such as `127.0.0.1.evil.example` must NOT slip through, so we
    // require the host to *parse* as a loopback IP (not merely look like one).
    host == "localhost"
        || host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Whether an `Origin` header value denotes a loopback origin (the local UI),
/// as opposed to a remote website mounting a cross-site WebSocket hijack.
pub(crate) fn is_local_origin(origin: &str) -> bool {
    // `Origin` is `scheme://host[:port]` (or the literal "null" for opaque
    // origins such as sandboxed iframes / file://, which we do NOT trust). We
    // only accept a loopback host.
    let Some((_scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host_header_is_loopback(hostport)
}

#[cfg(test)]
mod tests {
    use super::{bind_loopback, host_header_is_loopback, is_local_origin};

    #[test]
    fn loopback_origins_are_allowed() {
        for ok in [
            "http://localhost",
            "http://localhost:5173",
            "http://127.0.0.1:7420",
            "https://127.0.0.1:443",
            "http://[::1]:5173",
            "http://[::1]",
            "http://127.0.0.5:9000",
        ] {
            assert!(is_local_origin(ok), "{ok} should be allowed");
        }
    }

    #[test]
    fn remote_origins_are_refused() {
        for bad in [
            "https://evil.example",
            "https://evil.example:443",
            "http://attacker.test/path",
            "https://127.0.0.1.evil.example",
            "null",
            "http://[2606:4700:4700::1111]",
            "https://localhost.evil.example",
        ] {
            assert!(!is_local_origin(bad), "{bad} must be refused");
        }
    }

    #[test]
    fn loopback_hosts_are_allowed() {
        for ok in [
            "localhost",
            "localhost:7420",
            "127.0.0.1",
            "127.0.0.1:7420",
            "127.0.0.5:9000",
            "[::1]",
            "[::1]:7420",
        ] {
            assert!(host_header_is_loopback(ok), "{ok} should be allowed");
        }
    }

    #[test]
    fn non_loopback_hosts_are_refused() {
        for bad in [
            "evil.example",
            "evil.example:7420",
            "127.0.0.1.evil.example",
            "[2606:4700:4700::1111]:7420",
            "localhost.evil.example",
            "",
        ] {
            assert!(!host_header_is_loopback(bad), "{bad} must be refused");
        }
    }

    #[tokio::test]
    async fn port_zero_reports_the_os_assigned_port() {
        let (listener, addr) = bind_loopback(0, 20).await.expect("bind --port 0");
        assert_ne!(addr.port(), 0, "local_addr must report the real port");
        assert_eq!(
            addr.port(),
            listener.local_addr().expect("local_addr").port()
        );
    }
}
