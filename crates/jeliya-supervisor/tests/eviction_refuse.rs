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
