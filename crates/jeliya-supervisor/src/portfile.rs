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

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::SupervisorError;
use crate::generation::SeenGeneration;
use crate::redact::Redacted;

/// The daemon's `daemon.json` portfile name.
pub(crate) const PORTFILE_NAME: &str = "daemon.json";

/// A parsed portfile. Unknown fields are ignored (serde default), so a newer
/// daemon that adds informational fields still reads cleanly.
#[derive(Debug, Clone, Deserialize)]
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

/// The absolute portfile path within a data dir.
pub(crate) fn portfile_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PORTFILE_NAME)
}

/// Read and parse the portfile from `data_dir`, whole (a `0600` atomic
/// temp+rename write means a *readable* file is never half of one). `strict`
/// refuses a group/other-readable portfile on Unix (OQ-3 strict mode);
/// otherwise permissiveness is tolerated (the loopback threat model treats the
/// file as inherently local-user-readable), and the non-strict default is
/// warn-and-proceed — the supervisor logs nothing itself, leaving the decision
/// to the caller's policy.
pub(crate) fn read_portfile(data_dir: &Path, strict: bool) -> Result<Portfile, SupervisorError> {
    let path = portfile_path(data_dir);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
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

    #[cfg(unix)]
    if strict {
        if let Err(why) = enforce_owner_only(&path) {
            return Err(SupervisorError::PortfileUnreadable { path, why });
        }
    }
    #[cfg(not(unix))]
    let _ = strict;

    serde_json::from_str::<Portfile>(&raw).map_err(|e| SupervisorError::PortfileUnreadable {
        path,
        why: format!("not a portfile: {e}"),
    })
}

/// On Unix, refuse a portfile any group/other bit can read: the token lives in
/// it, and a `0640`/`0644` portfile leaks the token to another local user.
/// Defense-in-depth only — see the module note on the loopback threat model.
#[cfg(unix)]
fn enforce_owner_only(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|e| format!("could not stat the portfile: {e}"))?
        .permissions()
        .mode();
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
}
