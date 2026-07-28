//! Headless proof of the ownership and teardown contract (issue #159).
//!
//! None of this needs a display, which is the point: the parts of a desktop
//! shell most likely to be wrong — who owns the daemon, who may kill it, where
//! the token lives — are exactly the parts a screenshot cannot check.
//!
//! Every test here was confirmed to FAIL against a deliberately broken
//! supervisor before being trusted; see `README.md` "Deliberate regressions".

use std::path::{Path, PathBuf};

#[path = "../src/supervisor.rs"]
mod supervisor;

use supervisor::{resolve_jeliyad, start_or_adopt, Teardown};

fn data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "jeliya-spike-159-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp data dir");
    dir
}

fn binary() -> PathBuf {
    resolve_jeliyad().expect("a jeliyad binary (build it with `cargo build -p jeliyad`)")
}

/// Is a pid alive? `kill(pid, 0)` without actually signalling.
fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[tokio::test]
async fn a_spawned_daemon_is_owned_and_its_portfile_agrees_with_its_announcement() {
    let dir = data_dir("owned");
    let sidecar = start_or_adopt(&binary(), &dir).await.expect("start");

    assert!(sidecar.is_owned(), "a daemon we spawned must be owned");
    assert_eq!(sidecar.portfile.schema, 1);
    assert_eq!(sidecar.portfile.protocol, 1);
    assert!(alive(sidecar.portfile.pid), "the daemon should be running");

    // The token is a 256-bit hex value and must never be empty — an empty
    // token would still "connect" against a daemon that skipped auth.
    assert_eq!(sidecar.token().len(), 64, "256-bit hex auth token");
    assert!(sidecar.token().chars().all(|c| c.is_ascii_hexdigit()));

    // The URL we would hand to a renderer carries no secret.
    assert!(
        !sidecar.ws_url().contains(sidecar.token()),
        "the displayable ws URL must not embed the token"
    );
    assert!(sidecar.ws_url().starts_with("ws://127.0.0.1:"), "loopback only");

    let pid = sidecar.portfile.pid;
    assert_eq!(
        sidecar.shutdown().await.expect("shutdown"),
        Teardown::Graceful,
        "an owned daemon must exit gracefully on stdin EOF, not be SIGKILLed"
    );

    for _ in 0..50 {
        if !alive(pid) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(!alive(pid), "an owned daemon must be gone after shutdown");
    assert!(
        !dir.join("daemon.json").exists(),
        "a graceful exit removes the portfile"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The test that matters most. A second shell on a live data dir must adopt
/// the incumbent, and must leave it running when it exits.
#[tokio::test]
async fn an_adopted_daemon_outlives_the_shell_that_adopted_it() {
    let dir = data_dir("adopted");
    let bin = binary();

    let owner = start_or_adopt(&bin, &dir).await.expect("first start");
    assert!(owner.is_owned());
    let daemon_pid = owner.portfile.pid;

    let guest = start_or_adopt(&bin, &dir).await.expect("second start");
    assert!(!guest.is_owned(), "the second shell must adopt, not own");
    assert_eq!(
        guest.portfile.pid, daemon_pid,
        "the guest must adopt the incumbent, not a daemon of its own"
    );
    assert_eq!(guest.portfile.port, owner.portfile.port);

    // The guest leaves. The daemon must not notice.
    assert_eq!(
        guest.shutdown().await.expect("guest shutdown"),
        Teardown::LeftRunning,
        "adopted shutdown must be a no-op"
    );
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        alive(daemon_pid),
        "an adopted daemon must survive the shell that adopted it"
    );
    assert!(
        dir.join("daemon.json").exists(),
        "and its portfile must still be there for the real owner"
    );

    // Now the actual owner leaves, and it may take the daemon with it.
    assert_eq!(
        owner.shutdown().await.expect("owner shutdown"),
        Teardown::Graceful
    );
    for _ in 0..50 {
        if !alive(daemon_pid) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(!alive(daemon_pid), "the owner's shutdown ends the daemon");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The portfile carries the auth token, so its mode is a security property,
/// not a detail. 0600 or the token is readable by every local account.
#[cfg(unix)]
#[tokio::test]
async fn the_portfile_is_not_readable_by_other_users() {
    use std::os::unix::fs::PermissionsExt;

    let dir = data_dir("perms");
    let sidecar = start_or_adopt(&binary(), &dir).await.expect("start");

    let mode = std::fs::metadata(dir.join("daemon.json"))
        .expect("portfile")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the portfile carries the token; it must be 0600");

    sidecar.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A shell that is killed outright — no graceful path — must still not leave a
/// daemon running. This is what `--supervised` buys, and it is the difference
/// between a crash and an orphaned process holding a port and a data dir.
#[tokio::test]
async fn killing_the_shell_takes_the_owned_daemon_with_it() {
    let dir = data_dir("parent-death");
    let sidecar = start_or_adopt(&binary(), &dir).await.expect("start");
    let daemon_pid = sidecar.portfile.pid;
    assert!(alive(daemon_pid));

    // Drop without calling shutdown(): the ChildStdin is dropped with the
    // struct, which is exactly what happens when the shell panics or is killed.
    drop(sidecar);

    for _ in 0..50 {
        if !alive(daemon_pid) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        !alive(daemon_pid),
        "closing the shell's end of stdin must end an owned daemon"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
