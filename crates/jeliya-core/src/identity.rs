//! Device identity persistence under `--data-dir`, mirroring the iroh-rooms
//! CLI's on-disk layout (IR-0101): a public `identity.json` profile and a
//! secret-bearing `identity.secret`, both owner-only (`0600`, dir `0700`).
//!
//! Seeds are stored plaintext under owner-only permissions (the SDK MVP threat
//! model); the secret file is only ever opened by [`SecretKeys::load`].

use std::fs::OpenOptions;
use std::io::{ErrorKind as IoErrorKind, Write};
use std::path::Path;

use iroh_rooms::identity::SigningKey;
use zeroize::{Zeroize, Zeroizing};

use crate::error::{CoreError, CoreResult, ErrorKind};

/// Public profile file name.
pub const IDENTITY_FILE: &str = "identity.json";
/// Secret seed file name (the ONLY file holding secrets).
pub const SECRET_FILE: &str = "identity.secret";
/// On-disk format version (mirrors the CLI's).
const PROFILE_VERSION: u32 = 1;
/// Ed25519 seed length.
const SEED_LEN: usize = 32;
/// Display name recorded in the profile — the daemon protocol has no name
/// parameter on `identity.create`, so a fixed local default is used.
const DEFAULT_NAME: &str = "jeliya";

/// The public identity profile (no secret bytes; safe to serialize/log).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Profile {
    /// On-disk format version.
    pub version: u32,
    /// Local display name.
    pub name: String,
    /// `sender_id` public key, lowercase hex (64 chars).
    pub identity_id: String,
    /// `device_id` public key, lowercase hex (64 chars).
    pub device_id: String,
    /// Creation time (ms since epoch).
    pub created_at_ms: u64,
}

/// The two secret signing keys backing the local identity. No
/// `Debug`/`Serialize`, so a stray format call cannot leak a seed.
pub struct SecretKeys {
    /// Signs the device binding (authorizes `device_id` under `sender_id`).
    pub identity: SigningKey,
    /// Signs events; signatures verify under `device_id`.
    pub device: SigningKey,
}

/// On-disk shape of `identity.secret`; zeroized after use.
#[derive(serde::Deserialize)]
struct SecretFile {
    version: u32,
    identity_secret: String,
    device_secret: String,
}

impl Zeroize for SecretFile {
    fn zeroize(&mut self) {
        self.identity_secret.zeroize();
        self.device_secret.zeroize();
    }
}

/// Create the data directory owner-only (`0700` on Unix).
pub fn ensure_dir(dir: &Path) -> CoreResult<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| CoreError::internal(format!("could not create {}: {e}", dir.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
            CoreError::internal(format!(
                "could not set permissions on {}: {e}",
                dir.display()
            ))
        })?;
    }
    Ok(())
}

/// Load the public profile, or `Ok(None)` if no identity exists yet.
pub fn load_profile(data_dir: &Path) -> CoreResult<Option<Profile>> {
    let path = data_dir.join(IDENTITY_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == IoErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(CoreError::internal(format!(
                "could not read {}: {err}",
                path.display()
            )))
        }
    };
    let profile = serde_json::from_slice(&bytes).map_err(|e| {
        CoreError::internal(format!("corrupt identity file {}: {e}", path.display()))
    })?;
    Ok(Some(profile))
}

/// Create a fresh identity (and device) keypair under `data_dir`.
///
/// # Errors
/// [`ErrorKind::IdentityExists`] if either identity file already exists.
pub fn create(data_dir: &Path) -> CoreResult<Profile> {
    ensure_dir(data_dir)?;
    let identity_path = data_dir.join(IDENTITY_FILE);
    let secret_path = data_dir.join(SECRET_FILE);
    if identity_path.exists() || secret_path.exists() {
        return Err(CoreError::new(
            ErrorKind::IdentityExists,
            format!("an identity already exists in {}", data_dir.display()),
        ));
    }

    let identity_key = SigningKey::generate();
    let device_key = SigningKey::generate();
    let profile = Profile {
        version: PROFILE_VERSION,
        name: DEFAULT_NAME.to_owned(),
        identity_id: identity_key.identity_key().to_string(),
        device_id: device_key.device_key().to_string(),
        created_at_ms: crate::now_ms(),
    };
    let profile_json = serde_json::to_vec(&profile)
        .map_err(|e| CoreError::internal(format!("could not encode identity.json: {e}")))?;

    // The only secret-bearing buffer; wiped before return.
    let mut secret_json = secret_file_contents(&identity_key, &device_key);
    let write = write_new_owner_only(&secret_path, secret_json.as_bytes())
        .and_then(|()| write_new_owner_only(&identity_path, &profile_json));
    secret_json.zeroize();
    write.map_err(|e| {
        // `create_new(true)` is the atomic guard: the exists() pre-check above
        // has a TOCTOU window, so two concurrent `identity.create` calls can both
        // clear it, and the loser's exclusive open fails with `AlreadyExists`.
        // That is the protocol's `identity_exists`, not an `internal` bug.
        if e.kind() == IoErrorKind::AlreadyExists {
            CoreError::new(
                ErrorKind::IdentityExists,
                format!("an identity already exists in {}", data_dir.display()),
            )
        } else {
            CoreError::internal(format!(
                "could not write identity files to {}: {e}",
                data_dir.display()
            ))
        }
    })?;
    Ok(profile)
}

impl SecretKeys {
    /// Load and cross-check the secret keys against the public profile.
    ///
    /// # Errors
    /// [`ErrorKind::IdentityMissing`] if no identity exists; internal errors on
    /// corruption or a secret/public mismatch. No seed bytes appear in errors.
    pub fn load(data_dir: &Path) -> CoreResult<Self> {
        let path = data_dir.join(SECRET_FILE);
        let mut bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == IoErrorKind::NotFound => {
                return Err(CoreError::new(
                    ErrorKind::IdentityMissing,
                    format!("no identity in {}", data_dir.display()),
                ));
            }
            Err(err) => {
                return Err(CoreError::internal(format!(
                    "could not read {}: {err}",
                    path.display()
                )))
            }
        };
        let parsed: Result<SecretFile, _> = serde_json::from_slice(&bytes);
        bytes.zeroize();
        let mut parsed = parsed.map_err(|_| {
            CoreError::internal(format!("identity files are corrupt: {}", path.display()))
        })?;
        let keys = Self::from_secret_file(&parsed);
        parsed.zeroize();
        let keys = keys?;

        // Consistency guard: seeds must reproduce the public profile.
        let profile = load_profile(data_dir)?.ok_or_else(|| {
            CoreError::new(
                ErrorKind::IdentityMissing,
                format!("no identity in {}", data_dir.display()),
            )
        })?;
        if keys.identity.identity_key().to_string() != profile.identity_id
            || keys.device.device_key().to_string() != profile.device_id
        {
            return Err(CoreError::internal(format!(
                "identity files are inconsistent (secret keys do not match identity.json) in {}",
                data_dir.display()
            )));
        }
        Ok(keys)
    }

    fn from_secret_file(file: &SecretFile) -> CoreResult<Self> {
        if file.version != PROFILE_VERSION {
            return Err(CoreError::internal(format!(
                "unsupported identity.secret version {}",
                file.version
            )));
        }
        Ok(Self {
            identity: signing_key_from_seed_hex(&file.identity_secret)?,
            device: signing_key_from_seed_hex(&file.device_secret)?,
        })
    }
}

/// Decode a 32-byte seed from lowercase hex; intermediates are zeroized.
fn signing_key_from_seed_hex(seed_hex: &str) -> CoreResult<SigningKey> {
    let mut raw =
        hex::decode(seed_hex).map_err(|_| CoreError::internal("secret seed is not valid hex"))?;
    let key = if let Ok(seed) = <[u8; SEED_LEN]>::try_from(raw.as_slice()) {
        let seed = Zeroizing::new(seed);
        SigningKey::from_seed(&seed)
    } else {
        raw.zeroize();
        return Err(CoreError::internal("secret seed has the wrong length"));
    };
    raw.zeroize();
    Ok(key)
}

/// Build the `identity.secret` body; the caller must zeroize the result.
fn secret_file_contents(identity_key: &SigningKey, device_key: &SigningKey) -> String {
    let identity_seed = identity_key.to_seed();
    let device_seed = device_key.to_seed();
    let mut identity_hex = hex::encode(identity_seed.as_slice());
    let mut device_hex = hex::encode(device_seed.as_slice());
    let contents = format!(
        "{{\"version\":{PROFILE_VERSION},\"identity_secret\":\"{identity_hex}\",\
         \"device_secret\":\"{device_hex}\"}}\n"
    );
    identity_hex.zeroize();
    device_hex.zeroize();
    contents
}

/// Create `path` exclusively with owner-only permissions and write `bytes`.
fn write_new_owner_only(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::{create, load_profile, SecretKeys, IDENTITY_FILE, SECRET_FILE};
    use crate::error::ErrorKind;
    use tempfile::tempdir;

    #[test]
    fn create_then_load_roundtrips() {
        let dir = tempdir().unwrap();
        let profile = create(dir.path()).unwrap();
        assert_eq!(profile.identity_id.len(), 64);
        assert_eq!(profile.device_id.len(), 64);
        assert_ne!(profile.identity_id, profile.device_id);
        let loaded = load_profile(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.identity_id, profile.identity_id);
        let keys = SecretKeys::load(dir.path()).unwrap();
        assert_eq!(
            keys.identity.identity_key().to_string(),
            profile.identity_id
        );
        assert_eq!(keys.device.device_key().to_string(), profile.device_id);
    }

    #[test]
    fn second_create_is_identity_exists() {
        let dir = tempdir().unwrap();
        create(dir.path()).unwrap();
        let err = create(dir.path()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::IdentityExists);
    }

    #[test]
    fn load_missing_secret_is_identity_missing() {
        let dir = tempdir().unwrap();
        // SecretKeys has no Debug (so a stray {:?} can never leak a seed);
        // unwrap the error side manually.
        let err = match SecretKeys::load(dir.path()) {
            Ok(_) => panic!("load must fail with no identity"),
            Err(err) => err,
        };
        assert_eq!(err.kind, ErrorKind::IdentityMissing);
    }

    #[test]
    fn load_profile_missing_is_none() {
        let dir = tempdir().unwrap();
        assert!(load_profile(dir.path()).unwrap().is_none());
    }

    #[test]
    fn secret_never_leaks_into_identity_json() {
        let dir = tempdir().unwrap();
        create(dir.path()).unwrap();
        let json = std::fs::read_to_string(dir.path().join(IDENTITY_FILE)).unwrap();
        assert!(!json.contains("identity_secret"));
        assert!(std::fs::read_to_string(dir.path().join(SECRET_FILE))
            .unwrap()
            .contains("identity_secret"));
    }

    #[cfg(unix)]
    #[test]
    fn files_are_owner_only() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempdir().unwrap();
        create(dir.path()).unwrap();
        for name in [IDENTITY_FILE, SECRET_FILE] {
            let mode = std::fs::metadata(dir.path().join(name)).unwrap().mode();
            assert_eq!(mode & 0o777, 0o600, "{name} must be 0600");
        }
    }
}
