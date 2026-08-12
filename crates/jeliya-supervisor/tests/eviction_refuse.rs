//! The security-critical eviction *refusal* path (#170, spec §6.7 / fault #14),
//! proven without a real `jeliyad`.
//!
//! The issue names a hard non-goal: an incumbent whose generation is
//! **incompatible** but whose ownership **cannot be proven** must be refused
//! with the `GenerationMismatch` — nothing may be signalled, evicted, or
//! respawned. "Only a proven-owned incumbent may be replaced" (an acceptance
//! criterion) is precisely this refusal, and it is the branch the transport
//! (#172) leans on before it is allowed to depend on this crate.
//!
//! Unlike the real-`jeliyad` matrix in `supervision.rs`, this case needs no live
//! daemon at all: a **stub binary** that only prints `already_running` drives the
//! public [`Supervisor::start_or_adopt`] into the adopt path, and a hand-written
//! mismatched-generation portfile pointing at a **dead port** makes the
//! agreement gate fail closed. So it lives here, always runs, and pins the
//! refusal end to end through the public API.
//!
//! Why the assertion is a real regression detector (spec §8 "deliberate
//! regressions"): the refuse branch is the *only* path that returns
//! `GenerationMismatch` once eviction is opted in. If the `prove_owned` guard
//! were removed — i.e. the crate signalled an unprovable incumbent anyway — the
//! call would instead `SIGTERM` the fabricated PID and surface a `Handshake`
//! error (`ESRCH`) or a `ShutdownTimedOut`, never `GenerationMismatch`. Asserting
//! the exact variant (and the incompatible axis it carries) fails loudly on that
//! regression.
//!
//! Unix-only: the eviction lever it guards (`sigterm_foreign`) and the stub's
//! `#!/bin/sh` shim are both Unix; Windows eviction is deferred (OQ-5).

#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use jeliya_supervisor::{Generation, Supervisor, SupervisorConfig, SupervisorError, Timeouts};

/// Short, bounded budgets so the dead-port health probe fails fast in CI rather
/// than waiting out the production 30 s spawn / 500 ms connect defaults.
fn short_timeouts() -> Timeouts {
    Timeouts {
        spawn: Duration::from_secs(5),
        health_connect: Duration::from_millis(80),
        health_read: Duration::from_millis(80),
        teardown: Duration::from_millis(200),
        evict: Duration::from_millis(200),
    }
}

/// A stub "daemon": prints one `already_running` announcement (with the PID/port
/// the portfile advertises, so the ready↔portfile agreement check passes) and
/// exits 0, which is exactly the adopt path's incumbent-already-serving verdict.
/// It never opens a socket, so nothing answers on `port`.
fn write_stub(path: &Path, pid: u32, port: u16) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        "#!/bin/sh\necho '{{\"event\":\"already_running\",\"pid\":{pid},\"port\":{port}}}'\nexit 0\n"
    );
    std::fs::write(path, script).expect("write stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// Bind and immediately release a loopback port: nothing listens there, so the
/// ownership probe (`prove_owned`) is refused a connection and returns `false`.
fn dead_loopback_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

/// A stub that prints `already_running` and then HANGS instead of exiting — the
/// misbehaving/fault-injected incumbent the bounded already-running wait must
/// survive. It never opens a socket; the long sleep outlives any test budget so
/// the supervisor's own timeout (not the process) is what ends the wait.
fn write_hanging_stub(path: &Path, pid: u32, port: u16) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        "#!/bin/sh\necho '{{\"event\":\"already_running\",\"pid\":{pid},\"port\":{port}}}'\nsleep 600\n"
    );
    std::fs::write(path, script).expect("write hanging stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// A stub that exits NON-ZERO without printing anything — jeliyad's lock-held
/// `wait_for_free_lock` path (the data dir is locked with no healthy daemon, so
/// it exits 1 silently). `start_or_adopt` must map this to the retryable
/// `Wedged`, not a generic handshake failure.
fn write_silent_exit_stub(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, "#!/bin/sh\nexit 1\n").expect("write silent stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// A spawned binary that exits non-zero WITHOUT announcing surfaces `Wedged`
/// (retryable), not `Handshake`. Red-before: without the exit-status check on
/// the read-announcement error path, the stdout EOF is a bare `Handshake`.
#[tokio::test]
async fn a_silent_nonzero_exit_surfaces_wedged_not_handshake() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-wedged-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");
    let stub = root.join("jeliyad-wedged");
    write_silent_exit_stub(&stub);

    let config = SupervisorConfig {
        data_dir: Some(data),
        binary: Some(stub),
        timeouts: short_timeouts(),
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");
    let result = sup.start_or_adopt().await;
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        matches!(result, Err(SupervisorError::Wedged)),
        "a silent non-zero exit must be Wedged (retryable), not Handshake; got: {result:?}"
    );
}

/// The P1 pre-spawn gate: a data dir holding a stale INCOMPATIBLE portfile
/// (v1, no live daemon) must fail closed with `GenerationMismatch` BEFORE the
/// v2 daemon is spawned — otherwise the fresh daemon overwrites `daemon.json`
/// with v2 fields and validation would see only the replacement and succeed,
/// silently discarding the clean-slate reset. Default config (no
/// `replace_incompatible`), so the gate — not the eviction path — is what fires.
#[tokio::test]
async fn a_stale_incompatible_portfile_is_refused_before_spawning() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-pregate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");

    // A stub that WOULD announce `ready` (a fresh daemon) if spawned — proving
    // the refusal happens BEFORE the spawn, not via the daemon.
    let stub = root.join("jeliyad-fresh");
    write_ready_stub(&stub);

    let config = SupervisorConfig {
        data_dir: Some(data.clone()),
        binary: Some(stub),
        // NOT opting into replacement — the default pure-adopter path.
        replace_incompatible: false,
        timeouts: short_timeouts(),
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");

    // A v1 (incompatible) portfile recording THIS dir, with no live daemon.
    let portfile_json = format!(
        r#"{{"schema":1,"pid":1,"port":9,"protocol":1,"data_dir":{data:?},"auth_token":"t"}}"#
    );
    std::fs::write(sup.data_dir().join("daemon.json"), &portfile_json).expect("write portfile");

    let result = sup.start_or_adopt().await;
    let _ = std::fs::remove_dir_all(&root);

    match result {
        Err(SupervisorError::GenerationMismatch { actual, .. }) => {
            assert_eq!(
                actual.protocol,
                Some(1),
                "the refusal must carry the incompatible portfile's protocol"
            );
        }
        other => panic!(
            "a stale incompatible portfile must be refused with GenerationMismatch \
             before spawning; got: {other:?}"
        ),
    }
}

/// The P1 corner: `replace_incompatible = true` must NOT license a silent
/// overwrite of a STALE incompatible portfile (no live daemon). With no live
/// incumbent the eviction path is never reached, so the gate must still fire —
/// `replace_incompatible` defers to eviction only for a genuinely LIVE incumbent.
#[tokio::test]
async fn a_stale_incompatible_portfile_is_refused_even_with_replace_enabled() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-pregate2-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");
    let stub = root.join("jeliyad-fresh");
    write_ready_stub(&stub);
    let dead_port = dead_loopback_port();

    let config = SupervisorConfig {
        data_dir: Some(data.clone()),
        binary: Some(stub),
        // Opted into replacement — but the incumbent is STALE (dead port), so the
        // gate, not a silent overwrite, is what must happen.
        replace_incompatible: true,
        timeouts: short_timeouts(),
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");
    let portfile_json = format!(
        r#"{{"schema":1,"pid":1,"port":{dead_port},"protocol":1,"data_dir":{data:?},"auth_token":"t"}}"#
    );
    std::fs::write(sup.data_dir().join("daemon.json"), &portfile_json).expect("write portfile");

    let result = sup.start_or_adopt().await;
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        matches!(result, Err(SupervisorError::GenerationMismatch { .. })),
        "a stale incompatible portfile must be refused even with replace_incompatible; got: {result:?}"
    );
}

/// A MALFORMED (truncated) `daemon.json` is evidence, not absence: the pre-spawn
/// gate must fail closed with the read error rather than spawn a fresh daemon
/// that silently overwrites it.
#[tokio::test]
async fn a_malformed_portfile_is_not_silently_overwritten() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-torn-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");
    let stub = root.join("jeliyad-fresh");
    write_ready_stub(&stub);
    // A torn write: valid-JSON prefix, no close.
    std::fs::write(data.join("daemon.json"), r#"{"pid": 1, "port":"#).expect("write torn portfile");

    let config = SupervisorConfig {
        data_dir: Some(data.clone()),
        binary: Some(stub),
        timeouts: short_timeouts(),
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");
    let result = sup.start_or_adopt().await;
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        matches!(result, Err(SupervisorError::PortfileUnreadable { .. })),
        "a malformed portfile must fail closed, not be overwritten; got: {result:?}"
    );
}

/// A stub that announces a valid `ready` line and then blocks — a "fresh
/// daemon". Used to prove the pre-spawn gate fires WITHOUT spawning it (if it
/// were spawned, the announcement would drive the owned path, not a refusal).
fn write_ready_stub(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(
        path,
        "#!/bin/sh\necho '{\"event\":\"ready\",\"pid\":424242,\"port\":9}'\nsleep 600\n",
    )
    .expect("write ready stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// Fault #15-adjacent: a spawned binary that announces `already_running` and
/// then never exits must NOT wedge `start_or_adopt` forever. The adopted-path
/// child wait is bounded by `Timeouts::spawn`; on expiry the owned probe child
/// is force-killed and `Wedged` is surfaced. Red-before/green-after: remove the
/// `timeout(self.timeouts.spawn, child.wait())` wrap and this test hangs for the
/// stub's full 600 s sleep (a CI timeout), never returning `Wedged`.
#[tokio::test]
async fn a_hanging_already_running_child_times_out_as_wedged() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-hang-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");

    let pid: u32 = 4_000_000_001;
    let port = dead_loopback_port();

    let stub = root.join("jeliyad-hang");
    write_hanging_stub(&stub, pid, port);

    // A SHORT spawn budget so the bounded wait fires quickly; the stub sleeps
    // far longer, so the supervisor's timeout is unambiguously what ends it.
    let timeouts = Timeouts {
        spawn: Duration::from_millis(600),
        ..short_timeouts()
    };
    let config = SupervisorConfig {
        data_dir: Some(data.clone()),
        binary: Some(stub.clone()),
        timeouts,
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");

    // A portfile matching the announced pid/port (compatible generation) so the
    // adopt path reaches the child wait rather than failing the earlier
    // ready↔portfile agreement or portfile read.
    let portfile_json = format!(
        r#"{{"pid":{pid},"port":{port},"protocol":2,"storage_generation":2,"data_dir":{data:?},"auth_token":"t"}}"#
    );
    std::fs::write(sup.data_dir().join("daemon.json"), &portfile_json).expect("write portfile");

    let started = std::time::Instant::now();
    let result = sup.start_or_adopt().await;
    let elapsed = started.elapsed();
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        matches!(result, Err(SupervisorError::Wedged)),
        "a hanging already_running child must surface Wedged, not hang; got: {result:?}"
    );
    // The bound actually fired near the 600ms spawn budget, not merely "under
    // 600s". A few seconds tolerates CI scheduling slop while still proving the
    // wait is tied to Timeouts::spawn rather than a looser hardcoded value.
    assert!(
        elapsed < Duration::from_secs(5),
        "the wait must be bounded by Timeouts::spawn (~600ms), not the stub's lifetime (took {elapsed:?})"
    );
}

#[tokio::test]
async fn fault14_unprovable_incompatible_incumbent_is_refused_without_signalling() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-f14-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");

    // A PID far above any real one and a port with nothing listening: the
    // refusal must not depend on either being live, and a high PID guarantees no
    // real process is ever touched even if the guard regressed.
    let dead_pid: u32 = 4_000_000_000;
    let dead_port = dead_loopback_port();

    let stub = root.join("jeliyad-stub");
    write_stub(&stub, dead_pid, dead_port);

    // Opt in to replacing a proven-owned incompatible incumbent (allow_evict).
    // The point of the test is that this opt-in still refuses when ownership
    // cannot be proven.
    let config = SupervisorConfig {
        data_dir: Some(data.clone()),
        binary: Some(stub.clone()),
        replace_incompatible: true,
        timeouts: short_timeouts(),
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");

    // A portfile that DECLARES an incompatible generation (protocol 1, no
    // storage_generation — the v1 shape) so the agreement gate fails closed on
    // the declared axis *before* any health probe (fault #6 ordering). No
    // ws/http, so the loopback gate is skipped. Written to the canonical data
    // dir the supervisor actually reads.
    let portfile_json = format!(
        r#"{{"pid":{dead_pid},"port":{dead_port},"protocol":1,"data_dir":{data:?},"auth_token":"t"}}"#
    );
    std::fs::write(sup.data_dir().join("daemon.json"), &portfile_json).expect("write portfile");

    let result = sup.start_or_adopt().await;
    let _ = std::fs::remove_dir_all(&root);

    match result {
        Err(SupervisorError::GenerationMismatch { expected, actual }) => {
            // The refusal carries the incompatible declared generation — proof
            // the gate short-circuited on the declared axis and fed the refuse
            // branch, rather than any health/eviction outcome.
            assert_eq!(
                expected,
                Generation::new(2, 2),
                "expected axis must be this build's generation"
            );
            assert_eq!(
                actual.protocol,
                Some(1),
                "refusal must carry the incumbent's incompatible protocol"
            );
            assert_eq!(
                actual.storage_generation, None,
                "the v1 incumbent declares no storage_generation"
            );
        }
        other => panic!(
            "an unprovable incompatible incumbent must be REFUSED with GenerationMismatch and \
             nothing signalled (spec §6.7 / fault #14); got: {other:?}"
        ),
    }
}

/// A stub that spawns a descendant INTO its own process group with stdio closed
/// (so it does not hold the announcement pipe), has the descendant record its
/// pid and then sleep far beyond the test, and finally exits NON-ZERO without
/// announcing — the faulty/overridden binary of fault #5. The busy-wait ensures
/// the pid is recorded before the leader exits, so the test can find it.
fn write_forking_stub(path: &Path, pidfile: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let pf = pidfile.display();
    let script = format!(
        "#!/bin/sh\n\
         sh -c 'exec >/dev/null 2>&1 <&-; echo $$ > \"{pf}\"; sleep 600' &\n\
         i=0\n\
         while [ ! -s \"{pf}\" ] && [ $i -lt 100 ]; do i=$((i+1)); sleep 0.02; done\n\
         exit 1\n"
    );
    std::fs::write(path, script).expect("write forking stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// Whether `pid` is still a live process (owned by this uid), via `kill -0`.
fn pid_is_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Fault #5 (process-group hygiene): a faulty binary that spawns a descendant
/// into the isolated process group and then exits non-zero WITHOUT announcing
/// must not leave that descendant alive holding the data-dir lock. The
/// reaped-leader early-exit arms sweep the group.
///
/// Red-before/green-after: without `kill_reaped_process_group` in the reaped
/// arms, `start_or_adopt` reaps only the leader and returns `Wedged`, leaving the
/// `sleep 600` descendant orphaned and alive; with the sweep it is SIGKILLed.
#[tokio::test]
async fn a_reaped_leader_that_spawned_a_descendant_reaps_the_group() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-forker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");
    let pidfile = root.join("descendant.pid");
    let stub = root.join("jeliyad-forker");
    write_forking_stub(&stub, &pidfile);

    let config = SupervisorConfig {
        data_dir: Some(data),
        binary: Some(stub),
        timeouts: short_timeouts(),
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");
    let result = sup.start_or_adopt().await;
    assert!(
        matches!(result, Err(SupervisorError::Wedged)),
        "a silent non-zero exit must surface Wedged; got: {result:?}"
    );

    let pid: u32 = std::fs::read_to_string(&pidfile)
        .expect("descendant recorded its pid")
        .trim()
        .parse()
        .expect("pid is a number");
    // Poll for the SIGKILL to land (a killed process reaps within ms; the parent
    // is gone so init reaps the zombie, after which `kill -0` reports dead).
    let mut alive = true;
    for _ in 0..40 {
        if !pid_is_alive(pid) {
            alive = false;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Best-effort cleanup of the descendant if the assertion is about to fail, so
    // a regression does not leak a 10-minute sleep into the CI runner.
    if alive {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        !alive,
        "the descendant must be reaped with the process group, not orphaned (pid {pid})"
    );
}

/// A background loopback listener that answers `/api/health` as a LIVE daemon
/// proving `pid` with an INCOMPATIBLE (protocol 1) generation. It loops so the
/// gate's `prove_owned` probe (and any re-probe) is answered; the returned
/// handle is aborted by the test once the supervisor has run.
async fn spawn_incompatible_health(pid: u32) -> (u16, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind health");
    let port = listener.local_addr().expect("addr").port();
    let handle = tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            // Drain the request (best-effort), then answer a v1 health body that
            // proves the incumbent PID on an incompatible protocol.
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let body = format!("{{\"ok\":true,\"pid\":{pid},\"protocol\":1}}");
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    (port, handle)
}

/// The gate→spawn race, resolved per spec §12 / §6.7 case 13: when
/// `replace_incompatible` DEFERS to eviction for a proven-LIVE incompatible
/// incumbent, that incumbent can exit in the window between the `prove_owned`
/// probe and the `spawn`. Our own fresh daemon then acquires the directory and
/// announces `ready` (not `already_running`). That daemon is the bundled
/// binary — it serves THIS build's generation — so the deferred path must NOT
/// short-circuit on the vanished incumbent's stale declared generation; it must
/// hand the fresh daemon to `finish_owned`, whose served-generation/identity
/// gate governs it. A real fresh daemon serving the expected generation is then
/// ADOPTED (the incumbent evicted itself; end state = case 13's "new daemon
/// matches expected generation"); the supervisor gates on the SERVED generation
/// and never migrates on-disk state (§12 → #185).
///
/// This stub announces a FOREIGN pid, so `finish_owned`'s child-PID identity
/// check is what fires: the result is a `Handshake` identity error, NOT a
/// `GenerationMismatch` carrying the incumbent's protocol 1. Red-before/
/// green-after: with the earlier deferred-generation short-circuit on the
/// `ready` branch the call returned `GenerationMismatch { protocol: 1 }`;
/// governed by `finish_owned` it is a `Handshake`. Asserting "not
/// GenerationMismatch, and a PID-identity Handshake" pins the corrected contract.
#[tokio::test]
async fn a_live_incompatible_incumbent_freeing_the_dir_before_spawn_defers_to_finish_owned() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-toctou-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");

    // A stub that announces `ready` (our OWN fresh daemon) — the race outcome:
    // the incumbent freed the dir, so the spawn is not `already_running`.
    let stub = root.join("jeliyad-fresh");
    write_ready_stub(&stub);

    // A live incumbent whose health proves an incompatible generation, so the
    // gate's `prove_owned` succeeds and it DEFERS to eviction.
    let incumbent_pid: u32 = 4_000_000_002;
    let (live_port, health) = spawn_incompatible_health(incumbent_pid).await;

    let config = SupervisorConfig {
        data_dir: Some(data.clone()),
        binary: Some(stub),
        replace_incompatible: true,
        timeouts: short_timeouts(),
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");

    // A v1 (incompatible) portfile recording THIS dir and pointing at the LIVE
    // health server, so the gate proves the incumbent live and defers.
    let portfile_json = format!(
        r#"{{"pid":{incumbent_pid},"port":{live_port},"protocol":1,"data_dir":{data:?},"auth_token":"t"}}"#
    );
    std::fs::write(sup.data_dir().join("daemon.json"), &portfile_json).expect("write portfile");

    let result = sup.start_or_adopt().await;
    health.abort();
    let _ = std::fs::remove_dir_all(&root);

    // Must NOT refuse on the vanished incumbent's stale declared generation…
    assert!(
        !matches!(result, Err(SupervisorError::GenerationMismatch { .. })),
        "the deferred path must be governed by finish_owned's served-generation gate, \
         not short-circuited on the vanished incumbent's declared generation; got: {result:?}"
    );
    // …instead `finish_owned` runs and this foreign-pid stub fails its child-PID
    // identity check — proving a real fresh daemon serving the expected
    // generation would reach adoption (§6.7 case 13).
    match result {
        Err(SupervisorError::Handshake(message)) => {
            assert!(
                message.contains("pid"),
                "the refusal must be finish_owned's PID-identity check; got: {message}"
            );
        }
        other => panic!(
            "a deferred live-incompatible race that returns `ready` must be validated by \
             finish_owned (a PID-identity Handshake for this foreign-pid stub); got: {other:?}"
        ),
    }
}

/// The INVERSE portfile/health skew (round-18): a fresh portfile DECLARES the
/// EXPECTED generation, but the PID-bound HEALTH response advertises an
/// INCOMPATIBLE served generation. `validate_portfile` returns `GenerationMismatch`
/// on the served axis, so the eviction arm is entered — and it must EVICT, because
/// `prove_owned_incompatible` proves BOTH the PID and the served incompatibility.
/// The eviction decision must NOT additionally require the DECLARATION to mismatch
/// (the bug: `bound_and_incompatible` did, so this skew fell through to a bare
/// refusal even under `replace_incompatible`).
///
/// Proven with a huge (unsignalable) incumbent pid: eviction is ATTEMPTED, so
/// `sigterm_foreign` refuses the invalid pid with a `Handshake`, NOT the
/// `GenerationMismatch` a non-eviction returns. Red-before: with the declared-
/// mismatch precondition the result was `GenerationMismatch`; green-after it is a
/// non-`GenerationMismatch` eviction attempt.
#[tokio::test]
async fn a_compatible_declaration_with_incompatible_served_generation_is_evicted() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-invskew-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");

    // Health serves protocol 1 (incompatible) and proves the incumbent PID.
    let incumbent_pid: u32 = 4_000_000_004;
    let (live_port, health) = spawn_incompatible_health(incumbent_pid).await;

    // A stub that announces `already_running` for the LIVE incumbent (its pid/port
    // must match the portfile so `finish_adopted`'s handshake passes and the adopt
    // path — where the eviction arm lives — is entered).
    let stub = root.join("jeliyad-stub");
    write_stub(&stub, incumbent_pid, live_port);

    let config = SupervisorConfig {
        data_dir: Some(data.clone()),
        binary: Some(stub),
        replace_incompatible: true,
        timeouts: short_timeouts(),
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");

    // The portfile DECLARES the expected generation (protocol 2, storage 2) — only
    // the SERVED generation is incompatible.
    let portfile_json = format!(
        r#"{{"pid":{incumbent_pid},"port":{live_port},"protocol":2,"storage_generation":2,"data_dir":{data:?},"auth_token":"t"}}"#
    );
    std::fs::write(sup.data_dir().join("daemon.json"), &portfile_json).expect("write portfile");

    let result = sup.start_or_adopt().await;
    health.abort();
    let _ = std::fs::remove_dir_all(&root);

    // Eviction must be ATTEMPTED — a served-incompatible incumbent is un-adoptable
    // even though its declaration is fine — so the result is NOT a bare
    // GenerationMismatch refusal.
    assert!(
        !matches!(result, Err(SupervisorError::GenerationMismatch { .. })),
        "a compatible declaration with an incompatible SERVED generation must be evicted, \
         not refused on the declaration; got: {result:?}"
    );
    // The unsignalable (huge) incumbent pid makes the eviction attempt itself the
    // observable outcome (a Handshake refusing the invalid pid), never a clean adopt.
    assert!(
        result.is_err(),
        "the eviction attempt on an unsignalable pid must surface an error; got: {result:?}"
    );
}

/// A stub that emits a JSON-looking but MALFORMED announcement carrying a marker
/// (a string where a `port` number is required, so serde's own Display would echo
/// the value), then exits 0. Proves the surfaced error reproduces neither the raw
/// line nor the serde value.
fn write_leaky_stub(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(
        path,
        "#!/bin/sh\necho '{\"event\":\"ready\",\"pid\":1,\"port\":\"SUPERVISOR_SECRET_MARKER\"}'\nexit 0\n",
    )
    .expect("write leaky stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// A malformed announcement must NOT leak child-controlled content into the
/// surfaced error (spec §7.2 token-redaction boundary): a corrupted/overridden
/// daemon could stuff a token into the line. Red-before/green-after: the earlier
/// error embedded the raw line (`{line:?}`) — and echoing the serde error would
/// reproduce the mis-typed value — so the marker reached the error; now only the
/// content-free category/position is reported.
#[tokio::test]
async fn a_malformed_announcement_does_not_leak_its_contents() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-leak-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");
    let stub = root.join("jeliyad-leak");
    write_leaky_stub(&stub);

    let config = SupervisorConfig {
        data_dir: Some(data),
        binary: Some(stub),
        timeouts: short_timeouts(),
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");
    let result = sup.start_or_adopt().await;
    let _ = std::fs::remove_dir_all(&root);

    let rendered = format!("{result:?}");
    assert!(
        result.is_err(),
        "a malformed announcement must be an error; got: {rendered}"
    );
    assert!(
        !rendered.contains("SUPERVISOR_SECRET_MARKER"),
        "the announcement contents must never reach the surfaced error: {rendered}"
    );
}

/// A stub that announces `already_running`, spawns a stdio-closed descendant into
/// its process group, records its pid, then exits 0 — the adopted-path twin of
/// the forker stub.
fn write_adopt_forking_stub(path: &Path, pidfile: &Path, pid: u32, port: u16) {
    use std::os::unix::fs::PermissionsExt;
    let pf = pidfile.display();
    let script = format!(
        "#!/bin/sh\n\
         echo '{{\"event\":\"already_running\",\"pid\":{pid},\"port\":{port}}}'\n\
         sh -c 'exec >/dev/null 2>&1 <&-; echo $$ > \"{pf}\"; sleep 600' &\n\
         i=0\n\
         while [ ! -s \"{pf}\" ] && [ $i -lt 100 ]; do i=$((i+1)); sleep 0.02; done\n\
         exit 0\n"
    );
    std::fs::write(path, script).expect("write adopt forking stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// Fault #5, adopted path: a faulty binary that announces `already_running`,
/// spawns a descendant into the isolated group, and exits 0 must not orphan that
/// descendant. The adopted-path wait sweeps the group before adopting/returning.
/// Red-before/green-after: without the pre-wait pgid capture + sweep on this arm,
/// the `sleep 600` descendant survives `start_or_adopt`.
#[tokio::test]
async fn an_adopted_path_leader_that_spawned_a_descendant_reaps_the_group() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-adopt-forker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");
    let pidfile = root.join("descendant.pid");
    let announced_pid: u32 = 4_000_000_003;
    let dead_port = dead_loopback_port();
    let stub = root.join("jeliyad-adopt-forker");
    write_adopt_forking_stub(&stub, &pidfile, announced_pid, dead_port);

    let config = SupervisorConfig {
        data_dir: Some(data.clone()),
        binary: Some(stub),
        timeouts: short_timeouts(),
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");
    // A portfile matching the announced pid/port so the adopt path reaches the
    // wait arm; finish_adopted later fails the dead-port health probe, but the
    // group sweep runs in the wait arm before that.
    let portfile_json = format!(
        r#"{{"pid":{announced_pid},"port":{dead_port},"protocol":2,"storage_generation":2,"data_dir":{data:?},"auth_token":"t"}}"#
    );
    std::fs::write(sup.data_dir().join("daemon.json"), &portfile_json).expect("write portfile");

    let _ = sup.start_or_adopt().await;

    let descendant: u32 = std::fs::read_to_string(&pidfile)
        .expect("descendant recorded its pid")
        .trim()
        .parse()
        .expect("pid is a number");
    let mut alive = true;
    for _ in 0..40 {
        if !pid_is_alive(descendant) {
            alive = false;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if alive {
        let _ = std::process::Command::new("kill")
            .args(["-9", &descendant.to_string()])
            .status();
    }
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        !alive,
        "the adopted-path descendant must be reaped with the group (pid {descendant})"
    );
}

/// A background health server proving `pid` with a COMPATIBLE (protocol 2,
/// storage_generation 2) generation.
async fn spawn_compatible_health(pid: u32) -> (u16, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind health");
    let port = listener.local_addr().expect("addr").port();
    let handle = tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let body =
                format!("{{\"ok\":true,\"pid\":{pid},\"protocol\":2,\"storage_generation\":2}}");
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    (port, handle)
}

/// A live daemon that SERVES a compatible generation but whose portfile was
/// corrupted/replaced to DECLARE an incompatible one must NOT be evicted:
/// eviction proves incompatibility from HEALTH, not the portfile's declaration,
/// so a concurrent portfile swap cannot doom a still-live compatible incumbent.
///
/// Red-before/green-after: with the old PID-only `prove_owned`, the health proves
/// the pid → the compatible daemon's pid is SIGTERMed (an ESRCH `Handshake` here,
/// since the pid is not a real process); the served-generation guard
/// (`prove_owned_incompatible`) instead yields `GenerationMismatch`, signalling
/// nothing.
#[tokio::test]
async fn a_compatible_daemon_with_an_incompatible_portfile_is_not_evicted() {
    let root = std::env::temp_dir().join(format!(
        "jeliya-sup-declonly-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("data dir");

    // A representable, non-existent pid: a SIGTERM to it fails with ESRCH, so if
    // the eviction path were entered the result would be `Handshake`, not a
    // refusal — the red-before witness.
    let pid: u32 = 2_000_000_000;
    let (port, health) = spawn_compatible_health(pid).await;

    let stub = root.join("jeliyad-stub");
    write_stub(&stub, pid, port);

    let config = SupervisorConfig {
        data_dir: Some(data.clone()),
        binary: Some(stub),
        replace_incompatible: true,
        timeouts: short_timeouts(),
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");

    // The portfile DECLARES an incompatible generation (protocol 1) at the live,
    // COMPATIBLE-serving pid/port.
    let portfile_json = format!(
        r#"{{"pid":{pid},"port":{port},"protocol":1,"data_dir":{data:?},"auth_token":"t"}}"#
    );
    std::fs::write(sup.data_dir().join("daemon.json"), &portfile_json).expect("write portfile");

    let result = sup.start_or_adopt().await;
    health.abort();
    let _ = std::fs::remove_dir_all(&root);

    match result {
        Err(SupervisorError::GenerationMismatch { .. }) => {}
        other => panic!(
            "a compatible-serving daemon must not be evicted on a declared-only mismatch \
             (nothing signalled); got: {other:?}"
        ),
    }
}
