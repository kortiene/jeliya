//! The supervisor's **own** tolerant portfile deserializer.
//!
//! Like the #159 spike and the retired Dart supervisor, the crate re-declares
//! the portfile rather than importing `jeliyad`'s struct — the whole point is
//! to prove an independent client can speak the contract (spec §5.4, OQ-2). It
//! reuses `jeliya_api::Limits` for the one field that has a single canonical
//! shape, so the discovery object is defined once.
//!
//! Field policy:
//! - **Hard-required** (missing → the portfile is unreadable, treated as a torn
//!   write): `pid`, `port`, `auth_token`, `data_dir`.
//! - **Generation axes** (`protocol`, `storage_generation`): parsed as
//!   `Option`, because a v1 portfile omits `storage_generation` and must reach
//!   the generation gate as a *mismatch* (fail-closed, clean-slate), not be
//!   rejected as a torn write. Absence there is refusal, never a default (§D1).
//! - **Optional/informational**: `http`, `ws`, `version`, `min_protocol`,
//!   `limits`, `started_at_ms` — parsed tolerantly.
//! - `schema`, if present, is **ignored**: a v1 portfile is caught by the
//!   generation check, not by a schema number.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::SupervisorError;
use crate::generation::SeenGeneration;
use crate::redact::Redacted;

/// The daemon's `daemon.json` portfile name.
pub(crate) const PORTFILE_NAME: &str = "daemon.json";

/// A parsed portfile. Unknown fields are ignored (serde default), so a newer
/// daemon that adds informational fields still reads cleanly.
///
/// `Debug` is MANUAL, not derived: every field except `auth_token` is
/// attacker-influenced content read from `daemon.json`, and a syntactically valid
/// but corrupted portfile can carry the bearer token in a swapped `ws`/`http`/
/// `data_dir`/`version` field. A derived `Debug` would print those verbatim,
/// defeating the crate's all-`Debug` token-redaction boundary (§7.2) that
/// `Redacted<String>` gives `auth_token` alone. The manual impl withholds every
/// child-controlled STRING (keeping the safe numeric fields for diagnostics).
#[derive(Clone, Deserialize)]
pub struct Portfile {
    /// The advertised daemon PID.
    pub pid: u32,
    /// The advertised loopback port.
    pub port: u16,
    /// The advertised protocol generation, or `None` if absent (a pre-v2
    /// daemon). Absence is routed to the generation gate as a mismatch.
    #[serde(default)]
    pub protocol: Option<u64>,
    /// The advertised storage generation, or `None` if absent.
    #[serde(default)]
    pub storage_generation: Option<u64>,
    /// The advertised HTTP base, if present. Validated for loopback.
    #[serde(default)]
    pub http: Option<String>,
    /// The advertised WS endpoint, if present. Validated for loopback.
    #[serde(default)]
    pub ws: Option<String>,
    /// The daemon's own record of its data dir. Kept because the portfile is
    /// `0600` (a reader has proved local read access), but **not** used as the
    /// identity binding — identity binds through the portfile's *location* plus
    /// the PID-on-advertised-port health proof (spec §4 D2). Required only as a
    /// torn-write signal.
    pub data_dir: String,
    /// The per-start bearer token, redacted the instant it is read.
    pub auth_token: Redacted<String>,
    /// The daemon version string, if present.
    #[serde(default)]
    pub version: Option<String>,
    /// The minimum supported protocol, if present (informational).
    #[serde(default)]
    pub min_protocol: Option<u64>,
    /// The served limits, if present. Reuses the single `jeliya_api` definition.
    #[serde(default)]
    pub limits: Option<jeliya_api::Limits>,
    /// Daemon start time in ms since epoch, if present (informational).
    #[serde(default)]
    pub started_at_ms: Option<u64>,
}

impl Portfile {
    /// The generation this portfile *declares* (either axis may be absent).
    pub(crate) fn declared_generation(&self) -> SeenGeneration {
        SeenGeneration {
            protocol: self.protocol,
            storage_generation: self.storage_generation,
        }
    }

    /// The token, for the native transport only. Every read is greppable via
    /// [`Redacted::expose`].
    pub(crate) fn token(&self) -> &str {
        self.auth_token.expose()
    }
}

impl fmt::Debug for Portfile {
    /// Withholds every CHILD-CONTROLLED STRING (`http`/`ws`/`data_dir`/`version`),
    /// any of which a corrupted portfile could carry the bearer token in, while
    /// keeping the safe numeric/structured fields for diagnostics. `auth_token`
    /// stays wrapped in its own `Redacted` `Debug`. See the type note (§7.2).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A present string is shown as this marker (preserving `Some`/`None`, i.e.
        // whether the field was set) but never its value.
        const REDACTED: &str = "<redacted>";
        f.debug_struct("Portfile")
            .field("pid", &self.pid)
            .field("port", &self.port)
            .field("protocol", &self.protocol)
            .field("storage_generation", &self.storage_generation)
            .field("http", &self.http.as_ref().map(|_| REDACTED))
            .field("ws", &self.ws.as_ref().map(|_| REDACTED))
            .field("data_dir", &REDACTED)
            .field("auth_token", &self.auth_token)
            .field("version", &self.version.as_ref().map(|_| REDACTED))
            .field("min_protocol", &self.min_protocol)
            .field("limits", &self.limits)
            .field("started_at_ms", &self.started_at_ms)
            .finish()
    }
}

/// The absolute portfile path within a data dir.
pub(crate) fn portfile_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PORTFILE_NAME)
}

/// Bound on a single portfile read, off the async executor. A regular-file read
/// on a stalled network/FUSE mount can block indefinitely — `O_NONBLOCK` does NOT
/// make regular-file reads nonblocking — and `read_portfile` runs on the Tokio
/// executor via `TargetResolver::resolve` and the startup paths, so an unbounded
/// read would wedge an executor worker, defeating the crate's "every wait is
/// time-boxed" contract (§7.7). Mirrors `process::REAP_GRACE`'s stalled-mount
/// rationale.
const PORTFILE_READ_GRACE: Duration = Duration::from_secs(5);

/// [`read_portfile`], but bounded and OFF the async executor: the blocking file
/// I/O runs on `spawn_blocking` and the caller-visible wait is capped at
/// [`PORTFILE_READ_GRACE`]. A stalled mount can still leak the blocking-pool
/// thread, but the CALLER surfaces a `PortfileUnreadable` timeout rather than
/// hanging an executor worker. EVERY async caller must use this; the synchronous
/// [`read_portfile`] remains for the blocking body and for tests.
pub(crate) async fn read_portfile_bounded(
    data_dir: &Path,
    strict: bool,
) -> Result<Portfile, SupervisorError> {
    let owned_dir = data_dir.to_path_buf();
    let (tx, rx) = tokio::sync::oneshot::channel();
    // Run the (potentially stalled-mount) FS read on a DETACHED OS thread, not
    // `spawn_blocking`: Tokio JOINS its blocking pool on `Runtime::drop`, so a read
    // wedged on a hung NFS/FUSE mount would turn the caller-visible timeout into an
    // application-shutdown hang — a plain thread is not joined (the OS reaps it at
    // process exit). `Builder::spawn` maps a thread-creation failure (resource
    // exhaustion) to an error instead of panicking, so a fallible read stays fallible.
    let worker = std::thread::Builder::new()
        .name("jeliya-portfile-read".to_owned())
        .spawn(move || {
            let _ = tx.send(read_portfile(&owned_dir, strict));
        });
    if let Err(e) = worker {
        return Err(SupervisorError::PortfileUnreadable {
            path: portfile_path(data_dir),
            why: format!("could not start the portfile-read worker: {e}"),
        });
    }
    match tokio::time::timeout(PORTFILE_READ_GRACE, rx).await {
        Ok(Ok(result)) => result,
        // The worker dropped the sender without sending — only reachable if it
        // panicked mid-read; surface it as an unreadable portfile, never a hang.
        Ok(Err(_recv)) => Err(SupervisorError::PortfileUnreadable {
            path: portfile_path(data_dir),
            why: "portfile-read worker ended without a result (panicked?)".to_owned(),
        }),
        Err(_elapsed) => Err(SupervisorError::PortfileUnreadable {
            path: portfile_path(data_dir),
            why: format!(
                "portfile read exceeded the {}s bound (data dir on a stalled mount?)",
                PORTFILE_READ_GRACE.as_secs()
            ),
        }),
    }
}

/// Read and parse the portfile from `data_dir`, whole (a `0600` atomic
/// temp+rename write means a *readable* file is never half of one). `strict`
/// refuses a group/other-readable portfile on Unix (OQ-3 strict mode) as the
/// token-leak enforcement; the NON-strict default SILENTLY proceeds — it does
/// NOT check permissions and emits no warning, because this crate is headless
/// and links no logger, and the loopback threat model treats the portfile as
/// inherently local-user-readable. A caller that needs the permission guard on
/// a shared host must opt into `strict_portfile_perms`. (Non-strict is not
/// "warn-and-proceed": there is nowhere to warn.)
pub(crate) fn read_portfile(data_dir: &Path, strict: bool) -> Result<Portfile, SupervisorError> {
    use std::io::Read as _;

    // The real `daemon.json` is a few hundred bytes. A truncated/corrupt file,
    // or an attacker-planted one in a shared data dir, must surface the typed
    // `PortfileUnreadable` — not make `read_to_string` allocate the file's entire
    // length first. Because this runs on startup AND on every reconnect, an
    // accidental multi-gigabyte file would otherwise terminate the supervising
    // app before parsing can reject it. Read through a fixed cap (one byte past,
    // so an over-cap file is detected rather than silently truncated-then-parsed).
    const MAX_PORTFILE_BYTES: u64 = 64 * 1024;

    let path = portfile_path(data_dir);
    // Open NONBLOCKING (and no-symlink-follow on Unix), then validate the OPENED
    // descriptor with `fstat` (via `File::metadata`, which stats the fd — not the
    // pathname). A stat-then-open leaves a TOCTOU: another process could swap the
    // regular file for a FIFO between the two path operations, and the open would
    // block forever on the executor thread waiting for a writer. `O_NONBLOCK` makes
    // a FIFO open return a fd immediately; the fstat then rejects it BEFORE any
    // (blocking) read. `O_NOFOLLOW` refuses a symlinked `daemon.json` (the daemon
    // writes a plain file). Only a REGULAR file is read.
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW);
    }
    let file = match opts.open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(SupervisorError::PortfileMissing(path));
        }
        Err(err) => {
            return Err(SupervisorError::PortfileUnreadable {
                path,
                why: err.to_string(),
            });
        }
    };
    // One `fstat` on the descriptor we just opened, reused for BOTH the
    // regular-file check and (in strict mode) the permission check below. Binding
    // it here — rather than re-`stat`ing the path later — is what closes the
    // strict-mode TOCTOU: another process may swap `daemon.json` after this open,
    // but the mode we vet is the mode of the very inode we read the token from.
    let meta = match file.metadata() {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => {
            return Err(SupervisorError::PortfileUnreadable {
                path,
                why: "portfile is not a regular file (FIFO, device, or directory?) — refusing to read it".to_owned(),
            });
        }
        Err(err) => {
            return Err(SupervisorError::PortfileUnreadable {
                path,
                why: err.to_string(),
            });
        }
    };
    let mut raw = String::new();
    if let Err(err) = file.take(MAX_PORTFILE_BYTES + 1).read_to_string(&mut raw) {
        // A non-UTF-8 body reaches here as `InvalidData` — still "not a readable
        // portfile", never a panic.
        return Err(SupervisorError::PortfileUnreadable {
            path,
            why: err.to_string(),
        });
    }
    if raw.len() as u64 > MAX_PORTFILE_BYTES {
        return Err(SupervisorError::PortfileUnreadable {
            path,
            why: format!(
                "portfile exceeds the {MAX_PORTFILE_BYTES}-byte cap for this JSON contract"
            ),
        });
    }

    #[cfg(unix)]
    if strict {
        if let Err(why) = enforce_owner_only(&meta) {
            return Err(SupervisorError::PortfileUnreadable { path, why });
        }
    }
    #[cfg(not(unix))]
    let _ = (strict, &meta);

    serde_json::from_str::<Portfile>(&raw).map_err(|e| SupervisorError::PortfileUnreadable {
        path,
        // Report only the content-free classification + position, NOT serde's
        // Display: for a string supplied where a numeric field is expected it
        // echoes the raw value (`invalid type: string "…"`), which — if the
        // misplaced value is the bearer token — would reproduce the secret in a
        // `PortfileUnreadable` a caller may log (the token-redaction boundary,
        // §7.2). Mirrors the announcement parser.
        why: format!(
            "not a portfile ({:?} at line {} column {})",
            e.classify(),
            e.line(),
            e.column()
        ),
    })
}

/// On Unix, refuse a portfile any group/other bit can read: the token lives in
/// it, and a `0640`/`0644` portfile leaks the token to another local user.
/// Defense-in-depth only — see the module note on the loopback threat model.
///
/// Takes the [`Metadata`] already `fstat`ed from the OPEN descriptor, never a
/// fresh path `stat`: the mode vetted here must belong to the exact inode the
/// token was read from, or a swap of `daemon.json` between read and check would
/// let a group-readable file pass while its token is returned (the strict-mode
/// TOCTOU).
#[cfg(unix)]
fn enforce_owner_only(meta: &std::fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode();
    // Any bit outside owner rwx (0o700) means group/other can see the token.
    if mode & 0o077 != 0 {
        return Err(format!(
            "portfile mode {:o} is group/other-accessible; the token could leak (strict mode)",
            mode & 0o777
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2_portfile_json(port: u16) -> String {
        format!(
            r#"{{
              "schema": 7,
              "pid": 4242,
              "port": {port},
              "http": "http://127.0.0.1:{port}/",
              "ws": "ws://127.0.0.1:{port}/ws",
              "version": "0.6.1",
              "protocol": 2,
              "min_protocol": 2,
              "storage_generation": 2,
              "data_dir": "/home/u/.local/share/Jeliya",
              "auth_token": "deadbeef",
              "started_at_ms": 1000
            }}"#
        )
    }

    #[test]
    fn parses_a_v2_portfile_and_ignores_schema() {
        let pf: Portfile = serde_json::from_str(&v2_portfile_json(7420)).expect("parses");
        assert_eq!(pf.pid, 4242);
        assert_eq!(pf.port, 7420);
        assert_eq!(pf.protocol, Some(2));
        assert_eq!(pf.storage_generation, Some(2));
        assert_eq!(pf.token(), "deadbeef");
        // `schema` is present but ignored — it does not appear on the struct.
    }

    #[test]
    fn debug_of_a_portfile_never_reveals_the_token() {
        let pf: Portfile = serde_json::from_str(&v2_portfile_json(7420)).expect("parses");
        assert!(
            !format!("{pf:?}").contains("deadbeef"),
            "portfile Debug leaked the token"
        );
    }

    #[test]
    fn a_v1_portfile_parses_with_storage_generation_absent() {
        // A v1 portfile: has `schema` and `protocol: 1`, no `storage_generation`.
        // It must PARSE (so the generation gate can call it a mismatch), not be
        // rejected as a torn write.
        let v1 = r#"{
          "schema": 1, "pid": 1, "port": 5, "http": "http://127.0.0.1:5/",
          "ws": "ws://127.0.0.1:5/ws", "version": "0.5.0", "protocol": 1,
          "data_dir": "/d", "auth_token": "t", "started_at_ms": 0
        }"#;
        let pf: Portfile = serde_json::from_str(v1).expect("v1 parses");
        assert_eq!(pf.protocol, Some(1));
        assert_eq!(pf.storage_generation, None);
    }

    #[test]
    fn a_truncated_portfile_is_a_parse_error() {
        assert!(serde_json::from_str::<Portfile>(r#"{"pid": 1, "port":"#).is_err());
    }

    #[test]
    fn a_portfile_missing_a_hard_required_field_fails() {
        // No `pid` — a torn or foreign file, not a portfile.
        let missing_pid = r#"{"port": 5, "data_dir": "/d", "auth_token": "t"}"#;
        assert!(serde_json::from_str::<Portfile>(missing_pid).is_err());
    }

    #[test]
    fn a_portfile_missing_auth_token_fails() {
        let no_token = r#"{"pid":1,"port":5,"data_dir":"/d"}"#;
        assert!(serde_json::from_str::<Portfile>(no_token).is_err());
    }

    #[test]
    fn a_portfile_missing_data_dir_fails() {
        let no_dir = r#"{"pid":1,"port":5,"auth_token":"t"}"#;
        assert!(serde_json::from_str::<Portfile>(no_dir).is_err());
    }

    #[test]
    fn declared_generation_v2_portfile_carries_both_axes() {
        let pf: Portfile = serde_json::from_str(&v2_portfile_json(7420)).expect("parses");
        let gen = pf.declared_generation();
        assert_eq!(gen.protocol, Some(2));
        assert_eq!(gen.storage_generation, Some(2));
    }

    #[test]
    fn declared_generation_v1_portfile_has_absent_storage_generation() {
        let v1 = r#"{"schema":1,"pid":1,"port":5,"http":"http://127.0.0.1:5/",
            "ws":"ws://127.0.0.1:5/ws","version":"0.5.0","protocol":1,
            "data_dir":"/d","auth_token":"t","started_at_ms":0}"#;
        let pf: Portfile = serde_json::from_str(v1).expect("v1 parses");
        let gen = pf.declared_generation();
        assert_eq!(gen.protocol, Some(1));
        assert_eq!(
            gen.storage_generation, None,
            "v1 portfile must have storage_generation=None, not a default"
        );
    }

    #[test]
    fn portfile_with_fully_absent_generation_axes_has_both_none() {
        // A portfile that has no protocol or storage_generation at all (extremely
        // exotic but must parse and have both axes as None).
        let no_gen = r#"{"pid":1,"port":5,"data_dir":"/d","auth_token":"t"}"#;
        let pf: Portfile = serde_json::from_str(no_gen).expect("parses");
        let gen = pf.declared_generation();
        assert_eq!(gen.protocol, None);
        assert_eq!(gen.storage_generation, None);
    }

    #[test]
    fn optional_fields_tolerated_when_absent() {
        // http, ws, version, min_protocol, limits, started_at_ms are all optional.
        let minimal = r#"{"pid":1,"port":5,"protocol":2,"storage_generation":2,
            "data_dir":"/d","auth_token":"t"}"#;
        let pf: Portfile = serde_json::from_str(minimal).expect("parses without optional fields");
        assert_eq!(pf.http, None);
        assert_eq!(pf.ws, None);
        assert_eq!(pf.version, None);
        assert_eq!(pf.min_protocol, None);
        assert!(pf.limits.is_none());
        assert_eq!(pf.started_at_ms, None);
    }

    /// Fault case 20 (strict mode, Unix only): a group/other-readable portfile
    /// must be refused in strict mode to prevent token leakage to other local users.
    #[cfg(unix)]
    #[test]
    fn strict_mode_refuses_group_readable_portfile() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sup-pf-perms-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.json");
        std::fs::write(&path, v2_portfile_json(7420)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let result = read_portfile(&dir, true);
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::PortfileUnreadable { .. })),
            "strict mode must refuse a 0644 portfile; got: {result:?}"
        );
    }

    /// Fault case 20 counterpart: strict mode accepts a 0600 portfile (owner-only).
    #[cfg(unix)]
    #[test]
    fn strict_mode_accepts_owner_only_portfile() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sup-pf-perms2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.json");
        std::fs::write(&path, v2_portfile_json(7420)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let result = read_portfile(&dir, true);
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            result.is_ok(),
            "strict mode must accept a 0600 portfile; got: {result:?}"
        );
    }

    /// The strict-mode permission gate must vet the INODE THE TOKEN WAS READ FROM
    /// (the open descriptor), never a fresh `stat` of the path: otherwise a swap of
    /// `daemon.json` between the read and the check lets a group-readable file pass
    /// the gate while its token is returned from the ORIGINAL file (the strict-mode
    /// TOCTOU). We open an owner-only 0600 portfile, then rebind the PATH to a 0644
    /// decoy. `enforce_owner_only` receives the fd's metadata, so it still sees the
    /// original 0600 inode and accepts; a path re-`stat` (the pre-fix behaviour)
    /// would see the 0644 decoy and wrongly reject — so `via_fd.is_ok()` fails if
    /// anyone re-introduces the path stat, and the decoy assertion pins the
    /// divergence the test relies on.
    #[cfg(unix)]
    #[test]
    fn strict_perm_gate_follows_the_opened_descriptor_not_the_path() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let dir = std::env::temp_dir().join(format!("sup-pf-toctou-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.json");
        // The real, owner-only portfile we open and read the token from.
        std::fs::write(&path, v2_portfile_json(7420)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true)
            .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW);
        let file = opts.open(&path).unwrap();
        let meta = file.metadata().unwrap();

        // Swap the PATH to a group-readable decoy AFTER the open; the fd still
        // refers to the original 0600 inode.
        let decoy = dir.join("decoy.json");
        std::fs::write(&decoy, v2_portfile_json(7420)).unwrap();
        std::fs::set_permissions(&decoy, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::rename(&decoy, &path).unwrap();

        let via_fd = enforce_owner_only(&meta);
        let via_path_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            via_fd.is_ok(),
            "the gate must accept the 0600 inode it opened, regardless of a path swap; got: {via_fd:?}"
        );
        assert_eq!(
            via_path_mode, 0o644,
            "the path now resolves to the 0644 decoy — the divergence this test relies on"
        );
    }

    /// The manual `Debug` withholds every child-controlled STRING so a corrupted
    /// portfile that carried the bearer token in a swapped `ws`/`http`/`data_dir`/
    /// `version` field cannot leak it through diagnostics (§7.2). Red-before: a
    /// derived `Debug` printed those fields verbatim.
    #[test]
    fn debug_withholds_token_bearing_child_strings() {
        let secret = "s3cr3t".repeat(10);
        let pf = Portfile {
            pid: 1,
            port: 2,
            protocol: Some(2),
            storage_generation: Some(2),
            http: Some(secret.clone()),
            ws: Some(secret.clone()),
            data_dir: secret.clone(),
            auth_token: Redacted::new(secret.clone()),
            version: Some(secret.clone()),
            min_protocol: None,
            limits: None,
            started_at_ms: None,
        };
        let rendered = format!("{pf:?}");
        assert!(
            !rendered.contains(&secret),
            "Debug must not leak a token from any child-controlled string: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "Debug must mark redacted strings: {rendered}"
        );
        // The safe numeric fields survive for diagnostics.
        assert!(
            rendered.contains("pid: 1"),
            "safe numeric fields stay: {rendered}"
        );
    }

    /// `read_portfile_bounded` returns the same parsed portfile as the sync read
    /// for a well-formed file (the bound only fires on a stalled read, which is not
    /// deterministically reproducible in a unit test — the guarantee is structural:
    /// `spawn_blocking` + `timeout`).
    #[test]
    fn read_portfile_bounded_returns_the_parsed_portfile() {
        let dir = std::env::temp_dir().join(format!("sup-pf-bounded-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("daemon.json"), v2_portfile_json(7421)).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(read_portfile_bounded(&dir, false));
        std::fs::remove_dir_all(&dir).ok();
        let portfile = result.expect("a bounded read of a valid portfile returns it");
        assert_eq!(portfile.port, 7421);
    }

    /// An over-cap `daemon.json` is refused with `PortfileUnreadable` WITHOUT
    /// parsing it. The body is a VALID portfile preceded by 70 KiB of whitespace,
    /// so it is well-formed JSON that serde would happily parse — proving the cap,
    /// not a parse error, is what fires. Red-before/green-after: without the
    /// `take(cap+1)` bound `read_to_string` reads the whole file and serde returns
    /// `Ok(portfile)` (JSON ignores the leading whitespace), silently allocating
    /// the entire length; with the bound the read stops at the cap and the length
    /// check rejects it.
    #[test]
    fn an_oversized_portfile_is_refused_before_parsing() {
        let dir = std::env::temp_dir().join(format!("sup-pf-huge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let padded = format!("{}{}", " ".repeat(70 * 1024), v2_portfile_json(7420));
        std::fs::write(dir.join("daemon.json"), padded).unwrap();
        let result = read_portfile(&dir, false);
        std::fs::remove_dir_all(&dir).ok();
        match result {
            Err(SupervisorError::PortfileUnreadable { why, .. }) => assert!(
                why.contains("exceeds"),
                "an over-cap portfile must be rejected by the size cap, not a parse error; got: {why}"
            ),
            other => panic!("an over-cap portfile must be PortfileUnreadable; got: {other:?}"),
        }
    }

    /// A `daemon.json` that is NOT a regular file is refused BEFORE any blocking
    /// open/read. A directory stands in here (easy to create and non-blocking); a
    /// FIFO — the real hazard — would otherwise block `File::open` forever waiting
    /// for a writer, outside every configured timeout.
    #[test]
    fn a_non_regular_portfile_is_refused() {
        let dir = std::env::temp_dir().join(format!("sup-pf-nonreg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Make `daemon.json` itself a DIRECTORY (not a regular file).
        std::fs::create_dir_all(dir.join("daemon.json")).unwrap();
        let result = read_portfile(&dir, false);
        std::fs::remove_dir_all(&dir).ok();
        match result {
            Err(SupervisorError::PortfileUnreadable { why, .. }) => assert!(
                why.contains("regular"),
                "a non-regular portfile must be refused as not-a-regular-file; got: {why}"
            ),
            other => panic!("a non-regular portfile must be PortfileUnreadable; got: {other:?}"),
        }
    }

    /// A malformed portfile that supplies a string where a numeric field is
    /// expected must NOT echo that value into the `PortfileUnreadable` error: if
    /// the misplaced value were the bearer token, serde's Display (`invalid type:
    /// string "…"`) would leak it into a message a caller may log (§7.2).
    /// Red-before/green-after: with `format!("not a portfile: {e}")` the marker
    /// leaked; now only the content-free classification/position is reported.
    #[test]
    fn a_malformed_portfile_parse_error_does_not_echo_its_values() {
        let dir = std::env::temp_dir().join(format!("sup-pf-redact-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A marker where `port` (a number) is expected — serde would echo it.
        std::fs::write(
            dir.join("daemon.json"),
            r#"{"pid":1,"port":"PORTFILE_SECRET_MARKER","data_dir":"/d","auth_token":"t"}"#,
        )
        .unwrap();
        let result = read_portfile(&dir, false);
        std::fs::remove_dir_all(&dir).ok();
        match result {
            Err(SupervisorError::PortfileUnreadable { why, .. }) => assert!(
                !why.contains("PORTFILE_SECRET_MARKER"),
                "the malformed value must not reach the error message: {why}"
            ),
            other => panic!("a malformed portfile must be PortfileUnreadable; got: {other:?}"),
        }
    }
}
