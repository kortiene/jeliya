//! Headless integration tests against a real `jeliyad` binary (#170).
//!
//! These tests drive the acceptance-criteria fault matrix (spec §8) that
//! synthetic portfile unit tests cannot cover: cases that require a live daemon
//! with real process ownership, real portfiles, and real `/api/health` probes.
//!
//! **Every assertion here was confirmed to FAIL against a deliberately broken
//! supervisor before being trusted — the spec §8 "deliberate regressions"
//! discipline.** If a fault case is reachable without a live daemon, it lives in
//! a module-level `#[cfg(test)]` block, not here.
//!
//! ## Running
//!
//! ```sh
//! # Build jeliyad first (once per workspace change):
//! cargo build -p jeliyad
//! # Then run these tests (they pick up target/debug/jeliyad automatically):
//! cargo test -p jeliya-supervisor --test supervision
//! # Or with an explicit binary:
//! JELIYAD_BIN=/path/to/jeliyad cargo test -p jeliya-supervisor --test supervision
//! ```
//!
//! The tests skip gracefully when no binary is found so they do not break a
//! fresh checkout where `jeliyad` has not been compiled yet.

use std::path::{Path, PathBuf};
use std::time::Duration;

use jeliya_supervisor::{Generation, Supervisor, SupervisorConfig, Teardown};

// ── helpers ──────────────────────────────────────────────────────────────────

/// Generation the current workspace daemon speaks.
const GEN: Generation = Generation {
    protocol: 2,
    storage_generation: 2,
};

fn timeouts() -> jeliya_supervisor::Timeouts {
    jeliya_supervisor::Timeouts {
        spawn: Duration::from_secs(30),
        health_connect: Duration::from_millis(300),
        health_read: Duration::from_millis(500),
        teardown: Duration::from_secs(15),
        evict: Duration::from_secs(15),
    }
}

/// Create and return a unique temp data dir for the test.
fn data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jeliya-sup170-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp data dir");
    dir
}

/// Resolve the `jeliyad` binary. Returns `None` when not found so tests can
/// skip gracefully rather than panic on a fresh checkout.
fn binary() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("JELIYAD_BIN") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }
    // Honor CARGO_TARGET_DIR first: the tests routinely run under a custom
    // target dir (CI and local both do), and looking ONLY at the default
    // `target/debug` there means the binary is never found and these
    // real-daemon tests silently skip — passing while covering nothing. Fall
    // back to the default workspace `target/debug`.
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        candidates.push(PathBuf::from(dir).join("debug").join("jeliyad"));
    }
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("target/debug/jeliyad"),
    );
    candidates.into_iter().find(|p| p.is_file())
}

/// Build a [`Supervisor`] pointing at `dir` with the given binary.
fn supervisor(dir: &Path, bin: &Path) -> Supervisor {
    let config = SupervisorConfig {
        data_dir: Some(dir.to_owned()),
        binary: Some(bin.to_owned()),
        timeouts: timeouts(),
        ..SupervisorConfig::new(GEN)
    };
    Supervisor::resolve(config).expect("supervisor resolves")
}

/// Whether the process `pid` is still alive. Uses `/proc` on Linux; `kill -0`
/// via `std::process::Command` on other Unix.
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        // /proc/{pid} vanishes once the process is fully reaped.
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Poll every 50 ms until `pid` dies or `budget` elapses. Returns `true` when
/// the process is gone within the budget.
#[cfg(unix)]
async fn wait_dead(pid: u32, budget: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if !alive(pid) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ── fault #1 — owned lifecycle (AC-1, AC-3, AC-5 orderly path) ───────────────
//
// `start_or_adopt` on a fresh dir → `is_owned()`, portfile↔ready agree, token
// absent from `ws_url`, `shutdown` → `Graceful`, PID gone, portfile removed.

#[tokio::test]
async fn fault1_owned_lifecycle_end_to_end() {
    let Some(bin) = binary() else {
        eprintln!("fault1: no jeliyad binary — skipping (build with `cargo build -p jeliyad`)");
        return;
    };
    let dir = data_dir("f1-owned");
    let sup = supervisor(&dir, &bin);

    let sidecar = sup.start_or_adopt().await.expect("start_or_adopt");

    // AC-1: lifecycle distinguishable as Owned.
    assert!(sidecar.is_owned(), "a daemon we spawned must be owned");

    let pid = sidecar.pid();
    assert!(sidecar.port() > 0, "daemon must bind a non-zero port");

    // AC-3: fresh, validated target on every resolve.
    let resolver = sidecar.target_resolver();
    let target = resolver.resolve().await.expect("resolve");

    let url = target.ws_url().as_str();
    assert!(
        url.starts_with("ws://127.0.0.1:"),
        "WS URL must be loopback: {url}"
    );
    assert!(
        url.contains("v=2"),
        "URL must carry v= generation gate: {url}"
    );
    assert!(url.contains("sg=2"), "URL must carry sg= gate: {url}");

    let token = target.bearer().to_owned();
    assert_eq!(token.len(), 64, "token must be 256-bit hex (64 chars)");
    assert!(
        token.chars().all(|c| c.is_ascii_hexdigit()),
        "token must be hex"
    );
    assert!(
        !url.contains(&token),
        "the WS URL must never embed the token"
    );

    // Fault #21 (token redaction) exercised with a real token.
    let debug = format!("{target:?}");
    assert!(
        !debug.contains(&token),
        "DialTarget Debug leaked the real token: {debug}"
    );
    assert!(
        debug.contains("<redacted>"),
        "DialTarget Debug must show <redacted>"
    );

    // AC-5 orderly: graceful exit, portfile removed, PID gone.
    let teardown = sidecar.shutdown().await.expect("shutdown");
    assert_eq!(
        teardown,
        Teardown::Graceful,
        "owned daemon must exit gracefully on stdin EOF"
    );

    #[cfg(unix)]
    {
        let gone = wait_dead(pid, Duration::from_secs(5)).await;
        assert!(
            gone,
            "owned daemon PID {pid} must be gone after graceful shutdown"
        );
    }
    assert!(
        !dir.join("daemon.json").exists(),
        "a graceful exit removes the portfile"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── fault #2 + #3 — adopted lifecycle and adopted-never-stopped (AC-1, AC-2) ─
//
// Second `start_or_adopt` on a live dir → `!is_owned()`, adopts incumbent
// PID/port; `shutdown` → `LeftRunning`; incumbent survives; owner's shutdown
// ends it.

#[tokio::test]
async fn fault2_adopted_lifecycle_and_fault3_adopted_never_stopped() {
    let Some(bin) = binary() else {
        eprintln!("fault2/3: no jeliyad binary — skipping");
        return;
    };
    let dir = data_dir("f2-adopted");
    let sup = supervisor(&dir, &bin);

    let owner = sup.start_or_adopt().await.expect("owner start");
    assert!(owner.is_owned());
    let daemon_pid = owner.pid();
    let daemon_port = owner.port();

    // Second call on the same live dir must adopt.
    let guest = sup.start_or_adopt().await.expect("guest adopt");

    // AC-1: adopted lifecycle distinguishable.
    assert!(!guest.is_owned(), "second shell must adopt, not own");
    assert_eq!(
        guest.pid(),
        daemon_pid,
        "guest must bind the incumbent's PID"
    );
    assert_eq!(guest.port(), daemon_port);

    // AC-2: shutdown on an adopted sidecar is a deliberate no-op.
    let teardown = guest.shutdown().await.expect("guest shutdown");
    assert_eq!(
        teardown,
        Teardown::LeftRunning,
        "adopted shutdown must be LeftRunning"
    );

    // Fault #3: daemon is still alive after the guest left.
    tokio::time::sleep(Duration::from_millis(300)).await;
    #[cfg(unix)]
    assert!(
        alive(daemon_pid),
        "adopted daemon must survive the guest shutdown"
    );
    assert!(
        dir.join("daemon.json").exists(),
        "portfile must still exist after adopted shutdown"
    );

    let _ = owner.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

// ── fault #4 — fresh target per reconnect (AC-3) ─────────────────────────────
//
// Resolve, restart daemon (new port/token), resolve again → second DialTarget
// carries the new token; no stale value cached in the resolver.

#[tokio::test]
async fn fault4_target_resolver_returns_fresh_token_after_daemon_restart() {
    let Some(bin) = binary() else {
        eprintln!("fault4: no jeliyad binary — skipping");
        return;
    };
    let dir = data_dir("f4-fresh");
    let sup = supervisor(&dir, &bin);

    let sidecar = sup.start_or_adopt().await.expect("start");
    let resolver = sidecar.target_resolver();
    let t1 = resolver.resolve().await.expect("first resolve");
    let token1 = t1.bearer().to_owned();
    sidecar.shutdown().await.expect("first shutdown");

    tokio::time::sleep(Duration::from_millis(300)).await;

    let sidecar2 = sup.start_or_adopt().await.expect("second start");
    let t2 = resolver
        .resolve()
        .await
        .expect("second resolve after restart");
    let token2 = t2.bearer().to_owned();

    // Each start gets a fresh token; the resolver must return the new one.
    assert_ne!(
        token2, token1,
        "resolver must return the new token after restart (AC-3)"
    );

    let _ = sidecar2.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

// ── fault #16 — orderly app death (AC-5) ─────────────────────────────────────
//
// `Sidecar::shutdown()` → graceful; portfile removed; next start on same dir
// succeeds (no orphan lock or stale portfile from prior run).

#[tokio::test]
async fn fault16_orderly_shutdown_removes_portfile_and_allows_clean_restart() {
    let Some(bin) = binary() else {
        eprintln!("fault16: no jeliyad binary — skipping");
        return;
    };
    let dir = data_dir("f16-orderly");
    let sup = supervisor(&dir, &bin);

    let sidecar = sup.start_or_adopt().await.expect("start");
    let pid = sidecar.pid();

    let teardown = sidecar.shutdown().await.expect("shutdown");
    assert_eq!(teardown, Teardown::Graceful);

    #[cfg(unix)]
    {
        let gone = wait_dead(pid, Duration::from_secs(5)).await;
        assert!(gone, "PID {pid} must be gone after orderly shutdown");
    }
    assert!(
        !dir.join("daemon.json").exists(),
        "portfile must be removed after graceful exit"
    );

    // AC-5: no duplicate — the next start must find a clean dir.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let sidecar2 = sup
        .start_or_adopt()
        .await
        .expect("clean restart after orderly death");
    assert!(
        sidecar2.is_owned(),
        "clean restart must be owned, not adopted"
    );
    let _ = sidecar2.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

// ── fault #17 — abrupt app death (AC-5) ──────────────────────────────────────
//
// Drop `Sidecar` without calling `shutdown()` (simulates a panic or SIGKILL).
// The daemon must self-terminate via stdin EOF (`--supervised`).

#[tokio::test]
async fn fault17_abrupt_drop_terminates_owned_daemon_via_stdin_eof() {
    let Some(bin) = binary() else {
        eprintln!("fault17: no jeliyad binary — skipping");
        return;
    };
    let dir = data_dir("f17-abrupt");
    let sup = supervisor(&dir, &bin);

    let sidecar = sup.start_or_adopt().await.expect("start");
    let pid = sidecar.pid();

    // Drop without calling shutdown() — exactly what happens when the shell panics
    // or is SIGKILL-ed. The ChildStdin drops with the struct, sending EOF, and
    // `--supervised` makes the daemon self-terminate.
    drop(sidecar);

    #[cfg(unix)]
    {
        let gone = wait_dead(pid, Duration::from_secs(10)).await;
        assert!(
            gone,
            "abrupt drop must end daemon PID {pid} via stdin EOF (AC-5 / --supervised)"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ── fault #18 — attach-only adoption (AC-1, AC-2) ────────────────────────────
//
// `attach_to_running` against a daemon someone else spawned → adopts via
// portfile+health+generation; `shutdown` → `LeftRunning`.

#[tokio::test]
async fn fault18_attach_to_running_adopts_and_leaves_daemon_alive() {
    let Some(bin) = binary() else {
        eprintln!("fault18: no jeliyad binary — skipping");
        return;
    };
    let dir = data_dir("f18-attach");
    let sup = supervisor(&dir, &bin);

    let owner = sup.start_or_adopt().await.expect("owner start");
    assert!(owner.is_owned());
    let daemon_pid = owner.pid();

    // A second native client attaches without spawning.
    let attach_sup = supervisor(&dir, &bin);
    let guest = attach_sup
        .attach_to_running()
        .await
        .expect("attach_to_running");

    // AC-1: attach-only result is adopted.
    assert!(
        !guest.is_owned(),
        "attach_to_running must produce an adopted sidecar"
    );
    assert_eq!(
        guest.pid(),
        daemon_pid,
        "attached sidecar must report the incumbent's PID"
    );

    // The target resolver works from an attached sidecar.
    let resolver = guest.target_resolver();
    let target = resolver
        .resolve()
        .await
        .expect("resolve from attached sidecar");
    assert!(
        target.ws_url().as_str().starts_with("ws://127.0.0.1:"),
        "resolver from attached sidecar must produce a loopback URL"
    );

    // AC-2: shutdown on the attached sidecar is a no-op.
    let teardown = guest.shutdown().await.expect("attached shutdown");
    assert_eq!(teardown, Teardown::LeftRunning);

    tokio::time::sleep(Duration::from_millis(200)).await;
    #[cfg(unix)]
    assert!(
        alive(daemon_pid),
        "daemon must survive attach_to_running shutdown"
    );
    assert!(dir.join("daemon.json").exists());

    let _ = owner.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

// ── fault #12 — already-running race (AC-1) ───────────────────────────────────
//
// Two `start_or_adopt` calls on the same dir concurrently → exactly one owns,
// one adopts; exactly one daemon (the daemon's own lock prevents a second spawn).

#[tokio::test]
async fn fault12_concurrent_start_or_adopt_yields_exactly_one_daemon() {
    let Some(bin) = binary() else {
        eprintln!("fault12: no jeliyad binary — skipping");
        return;
    };
    let dir = data_dir("f12-race");
    let sup1 = supervisor(&dir, &bin);
    let sup2 = supervisor(&dir, &bin);

    let (r1, r2) = tokio::join!(sup1.start_or_adopt(), sup2.start_or_adopt());
    let s1 = r1.expect("first concurrent start");
    let s2 = r2.expect("second concurrent start");

    // Both must be bound to the same incumbent (same PID).
    assert_eq!(
        s1.pid(),
        s2.pid(),
        "both concurrent callers must bind the same daemon"
    );

    // Exactly one must be owned.
    let owned = [s1.is_owned(), s2.is_owned()]
        .iter()
        .filter(|&&o| o)
        .count();
    assert_eq!(
        owned, 1,
        "exactly one of the two concurrent calls must own the daemon (AC-1)"
    );

    let _ = s1.shutdown().await;
    let _ = s2.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

// ── NoBinary — fail-closed binary resolution ─────────────────────────────────
//
// No `jeliyad` required: verifies `NoBinary` surfaces when the binary is absent,
// without a live daemon — a unit-style check that belongs here because it
// validates the full `Supervisor::resolve` + `start_or_adopt` path.

#[tokio::test]
async fn no_binary_surfaces_no_binary_error_at_start_time() {
    let dir = data_dir("no-bin");
    let config = SupervisorConfig {
        data_dir: Some(dir.clone()),
        binary: Some(PathBuf::from("/definitely/not/a/jeliyad/binary/12345")),
        timeouts: timeouts(),
        ..SupervisorConfig::new(GEN)
    };
    let sup = Supervisor::resolve(config).expect("resolve succeeds even without a binary");
    let err = sup
        .start_or_adopt()
        .await
        .expect_err("start_or_adopt must fail when no binary is available");
    assert!(
        matches!(err, jeliya_supervisor::SupervisorError::NoBinary { .. }),
        "expected NoBinary, got: {err:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
