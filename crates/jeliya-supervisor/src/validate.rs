//! The agreement/skew logic shared by [`crate::TargetResolver::resolve`] and
//! the supervisor's adopt/attach paths: read the portfile, then require
//! loopback, a supported declared generation, a PID-bound health proof, and a
//! supported served generation — in that order.
//!
//! **Ordering is load-bearing** and pinned by two fault cases that a naive
//! order would confuse:
//!
//! - A portfile that *declares* an incompatible generation is a
//!   `GenerationMismatch` **before** any health probe (fault #6: a v1 portfile
//!   with no live daemon must fail-closed as a generation mismatch, not as a
//!   `Stale` "nothing answered"). Clean-slate: never adopt, never migrate.
//! - A portfile that declares a *supported* generation but has no matching
//!   healthy daemon is `Stale` (fault #7): on a spawn path a fresh daemon heals
//!   it; on a dial it is a transient retry.
//!
//! Eviction of a *proven-owned* incompatible incumbent (fault #13) does not go
//! through here — it re-proves ownership independently via
//! [`prove_owned`], because this function short-circuits on the declared
//! mismatch before it would reach the health step.

use std::path::Path;
use std::time::Duration;

use crate::error::SupervisorError;
use crate::generation::Generation;
use crate::health::{self, HealthReport};
use crate::portfile::{self, Portfile};
use crate::supervisor::Timeouts;
use crate::target::advertised_endpoint_is_loopback;

/// A portfile that passed every gate, plus the health response that proved it.
#[derive(Debug)]
pub(crate) struct Validated {
    pub portfile: Portfile,
    #[allow(dead_code)] // Held for callers that want the served generation; the
    // adopt path reads it, the resolve path does not.
    pub health: HealthReport,
}

/// Run the full gate for `data_dir` against `expected`. See the module note for
/// the ordering rationale.
pub(crate) async fn validate_portfile(
    data_dir: &Path,
    expected: Generation,
    strict_portfile_perms: bool,
    timeouts: &Timeouts,
) -> Result<Validated, SupervisorError> {
    let portfile = portfile::read_portfile(data_dir, strict_portfile_perms)?;

    // 0. Data-dir binding: the portfile must record THIS directory. A
    //    `daemon.json` copied/restored from another install keeps the original's
    //    pid/port/token/data_dir, so without this a copy's supervisor would pass
    //    validation against the ORIGINAL live daemon (and could SIGTERM it under
    //    eviction). Checked first, before any dial or signal (spec §4 D2:
    //    identity binds to the portfile's recorded location).
    if let Some(mismatch) = data_dir_mismatch(data_dir, &portfile.data_dir) {
        return Err(mismatch);
    }

    // 1. Loopback: a portfile advertising a non-loopback endpoint is refused
    //    before any dial (spec §6.4 step 2 / §7.1).
    if let Some(advertised) = non_loopback_endpoint(&portfile) {
        return Err(SupervisorError::NonLoopback { advertised });
    }

    // 2. Declared generation: the portfile's own `protocol`+`storage_generation`
    //    must match. Checked before health so a declared-incompatible portfile
    //    fails closed even with no live daemon (fault #6).
    if !expected.matches(portfile.declared_generation()) {
        return Err(SupervisorError::GenerationMismatch {
            expected,
            actual: portfile.declared_generation(),
        });
    }

    // 3. Health, PID-bound: the answering process on the advertised port must be
    //    the portfile's PID. Defeats a recycled port (unrelated listener → PID
    //    mismatch) and a recycled PID (no jeliyad on this port → probe fails).
    let Some(report) =
        health::probe_health(portfile.port, timeouts.health_connect, timeouts.health_read).await
    else {
        return Err(SupervisorError::Stale {
            port: portfile.port,
        });
    };
    if !report.proves_pid(portfile.pid) {
        return Err(SupervisorError::Stale {
            port: portfile.port,
        });
    }

    // 4. Served generation, fail-closed: health's generation must also match.
    //    Guards a portfile that lied and a daemon that changed generation under
    //    a reused port (the second line of defense the kernel's generation
    //    fence #168 backs up).
    if !expected.matches(report.advertised_generation()) {
        return Err(SupervisorError::GenerationMismatch {
            expected,
            actual: report.advertised_generation(),
        });
    }

    // 5. Bearer token shape (last, so it only rejects an otherwise-VALID, LIVE
    //    daemon's portfile — the corrupt-token case). jeliyad's token is 32
    //    CSPRNG bytes hex-encoded — 64 lowercase hex chars
    //    (`lifecycle::generate_token`). A corrupted token (empty / wrong length /
    //    non-hex) still parses as a `String`, so without this the resolver hands
    //    back an "apparently valid" target that every WebSocket handshake then
    //    rejects — a failure far from its cause. Refuse it here as unusable.
    if !token_is_well_formed(portfile.token()) {
        return Err(SupervisorError::PortfileUnreadable {
            path: portfile::portfile_path(data_dir),
            why: "auth_token is not a 64-char lowercase-hex bearer token".to_owned(),
        });
    }

    Ok(Validated {
        portfile,
        health: report,
    })
}

/// Whether the portfile's recorded `data_dir` fails to match `resolved` (the
/// directory this supervisor manages, already canonicalized in
/// `Supervisor::resolve`). Both sides are canonicalized before comparison so
/// `/var` vs `/private/var`, symlinks, and path spelling compare like-with-like
/// — the daemon canonicalizes its own recorded `data_dir` too. A recorded path
/// that no longer exists (canonicalize fails) is compared raw and, absent an
/// exact match, treated as a mismatch: a portfile pointing at a directory that
/// is not the one in hand is never trusted. Returns the error to raise, or
/// `None` when they match.
pub(crate) fn data_dir_mismatch(resolved: &Path, recorded: &str) -> Option<SupervisorError> {
    let recorded_path = Path::new(recorded);
    let recorded_canonical = recorded_path
        .canonicalize()
        .unwrap_or_else(|_| recorded_path.to_path_buf());
    if recorded_canonical == resolved || recorded_path == resolved {
        None
    } else {
        Some(SupervisorError::DataDirMismatch {
            recorded: recorded.to_owned(),
            resolved: resolved.to_path_buf(),
        })
    }
}

/// Whether `token` has jeliyad's bearer-token shape: exactly 64 lowercase hex
/// characters (32 CSPRNG bytes hex-encoded). Rejects empty, wrong-length, and
/// non-hex tokens that would authenticate no handshake.
fn token_is_well_formed(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The first advertised endpoint (`ws` then `http`) that is present but not
/// loopback, if any. Absent endpoints are fine — the resolver builds its own
/// `127.0.0.1` dial URL; the check exists to catch a portfile that *advertises*
/// a routable endpoint (fault #19).
fn non_loopback_endpoint(portfile: &Portfile) -> Option<String> {
    for candidate in [portfile.ws.as_deref(), portfile.http.as_deref()]
        .into_iter()
        .flatten()
    {
        if !advertised_endpoint_is_loopback(candidate) {
            return Some(candidate.to_owned());
        }
    }
    None
}

/// Prove — independently of generation — that the process at `portfile.pid` is
/// the live daemon on `portfile.port`. This is the ONLY gate that authorizes a
/// signal to an incumbent the supervisor does not own: eviction (spec §6.7)
/// calls it before any SIGTERM, so a recycled/dead PID or a foreign process
/// (fault #14) is never signalled. Returns `false` on any doubt.
pub(crate) async fn prove_owned(portfile: &Portfile, timeouts: &Timeouts) -> bool {
    match health::probe_health(portfile.port, timeouts.health_connect, timeouts.health_read).await {
        Some(report) => report.proves_pid(portfile.pid),
        None => false,
    }
}

/// Poll `/api/health` until the daemon at `pid`/`port` is gone (connection
/// refused, or a different/absent PID answers), bounded by `budget`. Returns
/// `true` if it went dark in time. Used to confirm an eviction actually took
/// effect before respawning (spec §6.7 / §6.9).
///
/// Note: a dark listener is **not** proof of completed cleanup — the daemon
/// drops its listener at the *start* of `graceful_shutdown` and only then
/// spends its room-close budget, removing `daemon.json` last. The eviction path
/// respawns into a fresh daemon that heals any leftover portfile, so listener
/// death is the right signal there; a caller that will *reuse the data dir*
/// must instead wait for [`wait_portfile_removed`].
pub(crate) async fn wait_health_dark(
    pid: u32,
    port: u16,
    budget: Duration,
    timeouts: &Timeouts,
) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        // Clamp the probe to the budget REMAINING: a probe whose per-attempt
        // connect/read timeout exceeds what is left — or a final probe that
        // starts just before the deadline against a stalled listener — would
        // otherwise run the full per-probe timeout PAST the deadline, so the
        // whole eviction/stop budget could overrun by one probe.
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let connect = timeouts.health_connect.min(remaining);
        let read = timeouts.health_read.min(remaining);
        let still_up = match health::probe_health(port, connect, read).await {
            Some(report) => report.proves_pid(pid),
            None => false,
        };
        if !still_up {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100).min(remaining)).await;
    }
}

/// Poll until the daemon's `daemon.json` under `data_dir` is gone, bounded by
/// `budget`. The daemon removes its portfile as the **final** step of
/// `graceful_shutdown` — after it has dropped its listener AND finished closing
/// rooms — so portfile absence is the completion signal a caller must see
/// before it may safely reuse or remove the data dir (the P2 the review names:
/// a bare health-dark check reports `Graceful` while the daemon is still writing
/// state and holding its lock). Returns `true` if it was removed in time.
pub(crate) async fn wait_portfile_removed(data_dir: &Path, budget: Duration) -> bool {
    let path = portfile::portfile_path(data_dir);
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        // `try_exists`, not `Path::exists`: the latter maps a stat ERROR (an
        // unreadable dir mid-shutdown) to `false` — the same as genuine absence —
        // which would report cleanup complete when it may not be. Only a
        // confirmed `Ok(false)` (the file is really gone) counts as removed; a
        // stat error keeps waiting and, on timeout, yields the honest
        // `ShutdownTimedOut`.
        if matches!(path.try_exists(), Ok(false)) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use crate::error::SupervisorError;
    use crate::generation::Generation;
    use crate::supervisor::Timeouts;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap()
    }

    /// Short health timeouts so tests that hit the "nothing answers" path do not
    /// wait 30 s for the spawn or 500 ms per health connect in CI.
    fn short() -> Timeouts {
        Timeouts {
            spawn: Duration::from_millis(200),
            health_connect: Duration::from_millis(50),
            health_read: Duration::from_millis(50),
            teardown: Duration::from_millis(200),
            evict: Duration::from_millis(200),
        }
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sup-validate-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_portfile(dir: &std::path::Path, json: &str) {
        let path = dir.join("daemon.json");
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }

    // --- non_loopback_endpoint unit tests (private fn, visible inside module) ---

    fn portfile_with_ws(ws: Option<&str>, http: Option<&str>) -> crate::portfile::Portfile {
        let ws_field = ws.map(|v| format!(r#","ws":"{v}""#)).unwrap_or_default();
        let http_field = http
            .map(|v| format!(r#","http":"{v}""#))
            .unwrap_or_default();
        let json = format!(
            r#"{{"pid":1,"port":9,"data_dir":"/d","auth_token":"t"{ws_field}{http_field}}}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn non_loopback_endpoint_catches_non_loopback_ws() {
        let pf = portfile_with_ws(Some("ws://1.2.3.4/ws"), None);
        assert!(non_loopback_endpoint(&pf).is_some());
    }

    #[test]
    fn non_loopback_endpoint_catches_non_loopback_http() {
        let pf = portfile_with_ws(None, Some("http://10.0.0.1/"));
        assert!(non_loopback_endpoint(&pf).is_some());
    }

    #[test]
    fn non_loopback_endpoint_accepts_loopback_ws() {
        let pf = portfile_with_ws(Some("ws://127.0.0.1:9/ws"), None);
        assert!(non_loopback_endpoint(&pf).is_none());
    }

    #[test]
    fn non_loopback_endpoint_returns_none_when_both_fields_absent() {
        let pf = portfile_with_ws(None, None);
        assert!(
            non_loopback_endpoint(&pf).is_none(),
            "no ws/http fields → no loopback violation"
        );
    }

    #[test]
    fn non_loopback_endpoint_lookalike_hostname_is_caught() {
        // "127.0.0.1.evil.example" does not parse as an IP and is not "localhost"
        // → refused exactly as the daemon's host_header_is_loopback logic would.
        let pf = portfile_with_ws(Some("ws://127.0.0.1.evil.example/ws"), None);
        assert!(non_loopback_endpoint(&pf).is_some());
    }

    // --- validate_portfile ordering tests (synthetic portfiles, no live daemon) ---

    /// Fault case 19: NonLoopback is returned BEFORE the generation gate and BEFORE
    /// any health probe — a non-loopback portfile must fail even with no daemon
    /// at all, and even if the generation would match.
    #[test]
    fn fault19_non_loopback_portfile_fails_before_health() {
        let dir = tmp_dir("f19");
        // `data_dir` records THIS dir so the data-dir binding check passes and
        // the loopback gate is what fires (the behavior under test).
        write_portfile(
            &dir,
            &format!(
                r#"{{"pid":1,"port":9,"protocol":2,"storage_generation":2,
                   "data_dir":{dir:?},"auth_token":"t","ws":"ws://evil.example/ws"}}"#
            ),
        );
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::NonLoopback { .. })),
            "expected NonLoopback, got: {result:?}"
        );
    }

    /// Fault case 6 / spec §D3 (ordering invariant): a v1 portfile (missing
    /// storage_generation) must yield GenerationMismatch BEFORE any health probe.
    /// If the error were Stale instead, a clean-slate daemon could be adopted by
    /// mistake on a spawn path that "heals" the Stale with a fresh spawn.
    #[test]
    fn fault6_v1_portfile_is_generation_mismatch_not_stale() {
        let dir = tmp_dir("f6");
        // v1 shape: schema, protocol:1, no storage_generation.
        write_portfile(
            &dir,
            &format!(
                r#"{{"schema":1,"pid":1,"port":9,"protocol":1,
                   "data_dir":{dir:?},"auth_token":"t"}}"#
            ),
        );
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::GenerationMismatch { .. })),
            "v1 portfile must be GenerationMismatch (not Stale); got: {result:?}"
        );
    }

    /// A portfile whose declared generation matches but has no listener → Stale.
    /// This is fault case 7. The key difference from fault 6: the generation gate
    /// passes, so the health probe runs and fails, which is the correct Stale path.
    #[test]
    fn fault7_compatible_portfile_with_no_listener_is_stale() {
        let dir = tmp_dir("f7");
        // Grab a free port and immediately release it; nothing will answer there.
        let free_port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let p = l.local_addr().unwrap().port();
            drop(l);
            p
        };
        write_portfile(
            &dir,
            &format!(
                r#"{{"pid":99999,"port":{free_port},"protocol":2,"storage_generation":2,
                   "data_dir":{dir:?},"auth_token":"t"}}"#
            ),
        );
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::Stale { .. })),
            "compatible portfile with no listener must be Stale; got: {result:?}"
        );
    }

    /// The data-dir-copy attack: a `daemon.json` copied from another install
    /// records the ORIGINAL's data dir (and a live pid/port). Validating it
    /// against a DIFFERENT resolved dir must fail closed with `DataDirMismatch`
    /// BEFORE any health probe or signal — otherwise the copy's supervisor would
    /// attach to (or, under eviction, SIGTERM) the original daemon. The recorded
    /// dir here points at a live loopback listener to prove the refusal does not
    /// depend on the daemon being dead: the binding gate fires first.
    #[test]
    fn a_portfile_recording_a_foreign_data_dir_is_refused_before_health() {
        let resolved = tmp_dir("ddbind-resolved");
        let foreign = tmp_dir("ddbind-foreign");
        // A live listener on the recorded port so a missing-listener path cannot
        // be what fails: the mismatch must short-circuit before the probe.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let live_port = listener.local_addr().unwrap().port();
        // The portfile records `foreign`, not `resolved`, but is written INTO
        // `resolved` (as a copy would be).
        write_portfile(
            &resolved,
            &format!(
                r#"{{"pid":1,"port":{live_port},"protocol":2,"storage_generation":2,
                   "data_dir":{foreign:?},"auth_token":"t"}}"#
            ),
        );
        let result = rt().block_on(validate_portfile(
            &resolved,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        drop(listener);
        std::fs::remove_dir_all(&resolved).ok();
        std::fs::remove_dir_all(&foreign).ok();
        assert!(
            matches!(result, Err(SupervisorError::DataDirMismatch { .. })),
            "a portfile recording a foreign data dir must be DataDirMismatch; got: {result:?}"
        );
    }

    /// Fault case 5 (validate path): missing portfile → PortfileMissing.
    #[test]
    fn fault5_missing_portfile_returns_portfile_missing() {
        let dir = tmp_dir("f5");
        // No daemon.json created.
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::PortfileMissing(_))),
            "missing portfile must be PortfileMissing; got: {result:?}"
        );
    }

    /// A truncated portfile (torn write) → PortfileUnreadable (validate path).
    #[test]
    fn truncated_portfile_is_portfile_unreadable() {
        let dir = tmp_dir("trunc");
        write_portfile(&dir, r#"{"pid": 1, "port":"#);
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::PortfileUnreadable { .. })),
            "truncated portfile must be PortfileUnreadable; got: {result:?}"
        );
    }

    /// A portfile with absent ws/http fields must not raise NonLoopback — the
    /// resolver builds its own 127.0.0.1 URL; these fields are informational only.
    /// The test still yields GenerationMismatch (not NonLoopback). `data_dir`
    /// records this dir so the check under test (loopback-skip) is reached, not
    /// short-circuited by the data-dir binding gate.
    #[test]
    fn portfile_without_ws_or_http_field_skips_loopback_check() {
        let dir = tmp_dir("nows");
        write_portfile(
            &dir,
            &format!(
                r#"{{"pid":1,"port":9,"protocol":1,"storage_generation":1,
                   "data_dir":{dir:?},"auth_token":"t"}}"#
            ),
        );
        let result = rt().block_on(validate_portfile(
            &dir,
            Generation::new(2, 2),
            false,
            &short(),
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            !matches!(result, Err(SupervisorError::NonLoopback { .. })),
            "no ws/http fields must not yield NonLoopback"
        );
        // And specifically NOT DataDirMismatch — the binding gate must accept a
        // portfile that records this very dir.
        assert!(
            !matches!(result, Err(SupervisorError::DataDirMismatch { .. })),
            "a portfile recording this dir must pass the data-dir binding gate"
        );
    }

    /// `wait_portfile_removed` returns true as soon as `daemon.json` is gone —
    /// the completion signal `stop_adopted` waits for before promising a
    /// `Graceful` teardown (a dark listener alone is premature: the daemon
    /// removes the portfile only after closing rooms).
    #[test]
    fn wait_portfile_removed_returns_true_once_the_portfile_is_gone() {
        let dir = tmp_dir("pf-gone");
        write_portfile(
            &dir,
            r#"{"pid":1,"port":9,"data_dir":"/d","auth_token":"t"}"#,
        );
        let path = dir.join("daemon.json");
        assert!(path.exists());
        let dir_for_task = dir.clone();
        let dir_for_wait = dir.clone();
        let result = rt().block_on(async move {
            // Remove the portfile after a short delay, as a graceful daemon
            // does at the END of its shutdown.
            let remover = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                std::fs::remove_file(dir_for_task.join("daemon.json")).unwrap();
            });
            let removed = wait_portfile_removed(&dir_for_wait, Duration::from_secs(2)).await;
            let _ = remover.await;
            removed
        });
        std::fs::remove_dir_all(&dir).ok();
        assert!(result, "must observe the portfile removal within budget");
    }

    /// `wait_portfile_removed` returns false when the portfile is never removed
    /// within the budget — so `stop_adopted` surfaces `ShutdownTimedOut` rather
    /// than a premature `Graceful` when cleanup stalls (the P2's honest verdict).
    #[test]
    fn wait_portfile_removed_times_out_when_the_portfile_persists() {
        let dir = tmp_dir("pf-stays");
        write_portfile(
            &dir,
            r#"{"pid":1,"port":9,"data_dir":"/d","auth_token":"t"}"#,
        );
        let result = rt().block_on(wait_portfile_removed(&dir, Duration::from_millis(200)));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            !result,
            "a portfile that never disappears must time out, not report removed"
        );
    }

    #[test]
    fn token_is_well_formed_accepts_only_64_lowercase_hex() {
        // 64 lowercase hex = 32 CSPRNG bytes hex-encoded (jeliyad's token).
        let good = "0123456789abcdef".repeat(4);
        assert_eq!(good.len(), 64);
        assert!(super::token_is_well_formed(&good));
        // Malformed: empty, too short, too long, uppercase, non-hex.
        assert!(!super::token_is_well_formed(""));
        assert!(!super::token_is_well_formed("deadbeef"));
        assert!(!super::token_is_well_formed(&"a".repeat(63)));
        assert!(!super::token_is_well_formed(&"a".repeat(65)));
        assert!(!super::token_is_well_formed(&"A".repeat(64))); // uppercase
        assert!(!super::token_is_well_formed(&format!(
            "g{}",
            "a".repeat(63)
        ))); // non-hex
    }

    /// prove_owned returns false when nothing answers on the advertised port.
    /// This is the gate that prevents signalling a recycled/foreign PID (fault #14).
    #[test]
    fn prove_owned_returns_false_when_nothing_answers() {
        let free_port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let p = l.local_addr().unwrap().port();
            drop(l);
            p
        };
        let pf: crate::portfile::Portfile = serde_json::from_str(&format!(
            r#"{{"pid":99999,"port":{free_port},"protocol":2,"storage_generation":2,
               "data_dir":"/d","auth_token":"t"}}"#
        ))
        .unwrap();
        let result = rt().block_on(prove_owned(&pf, &short()));
        assert!(
            !result,
            "prove_owned must be false when nothing answers; a recycled PID must not be signalled"
        );
    }
}
