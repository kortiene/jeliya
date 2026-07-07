//! `jeliyad` — the Jeliya daemon: a local-only WebSocket server at
//! `ws://127.0.0.1:<port>/ws` implementing `docs/PROTOCOL.md` over
//! `jeliya-core` (the sole consumer of the iroh-rooms SDK).
//!
//! Local-only by construction: the listener binds `127.0.0.1` and nothing
//! else — there is no flag to bind another interface, so the protocol's
//! "MUST refuse to bind non-loopback interfaces" holds trivially.

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
use tokio::sync::broadcast;

use jeliya_core::error::ErrorKind;
use jeliya_core::supervisor::RoomSupervisor;

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
}

/// Shared server state: the supervisor and the push fan-out channel.
///
/// The supervisor is shared as a plain `Arc` (no daemon-wide async mutex): its
/// own internal locks are held only for brief map operations, never across a
/// network `.await`, so one client's slow request can no longer freeze every
/// other client or the push loop.
#[derive(Clone)]
struct AppState {
    supervisor: Arc<RoomSupervisor>,
    data_dir: PathBuf,
    push_tx: broadcast::Sender<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let data_dir = args.data_dir.clone().unwrap_or_else(default_data_dir);
    let supervisor = match RoomSupervisor::new(data_dir.clone(), args.loopback) {
        Ok(sup) => sup,
        Err(err) => {
            eprintln!("error: could not initialize the data dir: {err}");
            std::process::exit(1);
        }
    };
    let (push_tx, _) = broadcast::channel(1024);
    let state = AppState {
        supervisor: Arc::new(supervisor),
        data_dir: data_dir.clone(),
        push_tx,
    };

    // Where the web UI is served from: an explicit --ui-dir wins; otherwise the
    // assets embedded at build time (present only in an `embed-ui` build).
    let ui = serve::UiSource::resolve(args.ui_dir.clone());

    // Bind loopback ONLY (see the module doc). On a port collision, scan a small
    // range upward rather than dying: a second launch (or a leftover daemon)
    // must not hard-fail a GUI start, and because the served UI is same-origin
    // the actual bound port propagates to the page automatically.
    let (listener, addr) = match bind_loopback(args.port, 20).await {
        Some(bound) => bound,
        None => {
            eprintln!(
                "error: no free loopback port in {}..={}",
                args.port,
                args.port.saturating_add(20)
            );
            std::process::exit(1);
        }
    };
    if addr.port() != args.port {
        eprintln!(
            "note: port {} was in use; bound {} instead",
            args.port,
            addr.port()
        );
    }

    if ui.is_serving() {
        println!(
            "jeliyad on http://{addr}/  (ws://{addr}/ws)  data dir: {}",
            data_dir.display()
        );
    } else {
        println!(
            "jeliyad listening on ws://{addr}/ws (data dir: {})",
            data_dir.display()
        );
    }

    tokio::spawn(push_loop(state.clone()));

    // Open the UI once we're bound and actually serving it (best-effort, never
    // fatal). Scripts and headless runs build without the UI (or pass --no-open)
    // so nothing pops a browser there.
    if ui.is_serving() && !args.no_open {
        let url = format!("http://{addr}/");
        if let Err(err) = webbrowser::open(&url) {
            eprintln!("note: could not open a browser ({err}); open {url} yourself");
        }
    }

    loop {
        match listener.accept().await {
            Ok((stream, _peer)) => {
                let state = state.clone();
                let ui = ui.clone();
                tokio::spawn(async move {
                    serve::handle_conn(stream, state, ui).await;
                });
            }
            Err(err) => {
                eprintln!("warning: accept failed: {err}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// The default per-user data directory (`~/Library/Application Support/Jeliya`,
/// `$XDG_DATA_HOME/Jeliya`, or `%APPDATA%\Jeliya`), falling back to a
/// cwd-relative dir only when no platform path is discoverable.
fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|dir| dir.join("Jeliya"))
        .unwrap_or_else(|| PathBuf::from("./.jeliya-data"))
}

/// Bind `127.0.0.1:port`, scanning up to `tries` ports upward past a collision.
/// Only `AddrInUse` advances the scan; any other bind error is fatal (`None`).
async fn bind_loopback(port: u16, tries: u16) -> Option<(TcpListener, SocketAddr)> {
    for candidate in port..=port.saturating_add(tries) {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, candidate));
        match TcpListener::bind(addr).await {
            Ok(listener) => return Some((listener, addr)),
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(err) => {
                eprintln!("error: could not bind {addr}: {err}");
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
                                eprintln!("warning: room-event pump error for {key}: {err}");
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
                Err(err) => eprintln!("warning: push reconcile failed for {room_str}: {err}"),
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

/// Whether an `Origin` header value denotes a loopback origin (the local UI),
/// as opposed to a remote website mounting a cross-site WebSocket hijack.
fn is_local_origin(origin: &str) -> bool {
    // `Origin` is `scheme://host[:port]` (or the literal "null" for opaque
    // origins such as sandboxed iframes / file://, which we do NOT trust). We
    // only accept a loopback host.
    let Some((_scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or(rest);
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

#[cfg(test)]
mod tests {
    use super::is_local_origin;

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
}
