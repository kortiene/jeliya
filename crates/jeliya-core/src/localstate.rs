//! Daemon-local JSON state under `--data-dir` (`state.json`): the known-rooms
//! index and local room display names.
//!
//! **Which side owns the room name?** The wire protocol *does* carry a name:
//! `room.created.room_name` is a signed field of the genesis event (verified
//! against the SDK's `RoomCreated` content). So the authoritative display name
//! is the genesis name once the room's log is (or has been) synced; this file
//! keeps (a) the index of rooms this daemon knows about and (b) an optional
//! local override — e.g. the `name` a joiner passed to `room.join` before the
//! genesis was pulled, or a rename that must stay local because the wire
//! protocol has no rename event.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{CoreError, CoreResult};
use crate::identity::ensure_dir;

/// State file name under the data dir.
pub const STATE_FILE: &str = "state.json";
/// On-disk format version.
const STATE_VERSION: u32 = 1;

/// Serialize the process-local read/modify/write transaction over `state.json`.
///
/// The daemon is single-instance per data directory, but it serves concurrent
/// RPCs. Atomic rename protects readers from partial JSON; it does not prevent
/// two writers from loading the same old state and then overwriting one
/// another. Every mutation crosses this lock so accepted-room provenance,
/// peer hints, and fetched-file records cannot be lost by a concurrent write.
static STATE_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Daemon-local metadata for one known room.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RoomMeta {
    /// Local display-name override (`None` ⇒ use the wire genesis name).
    pub name: Option<String>,
    /// When this daemon first learned about the room (ms since epoch).
    pub added_at_ms: u64,
    /// Known peer dial hints (`"<endpoint_id>@<ip:port,...>"`). Loopback mode
    /// has no discovery, so the managed room session can only dial peers it
    /// has an explicit address for: the `peers` a joiner passed to
    /// `room.join`/`room.open`, plus addresses harvested from live sessions.
    #[serde(default)]
    pub peer_hints: Vec<String>,
    /// Files this daemon fetched and verified locally, keyed by `file_<hex>`.
    #[serde(default)]
    pub fetched_files: BTreeMap<String, FetchedFileMeta>,
}

/// Daemon-local record of a verified file fetch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FetchedFileMeta {
    pub path: PathBuf,
    pub bytes: u64,
    pub fetched_at_ms: u64,
}

/// The whole daemon-local state file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LocalState {
    /// On-disk format version.
    pub version: u32,
    /// Known rooms, keyed by `blake3:<hex>` room id.
    pub rooms: BTreeMap<String, RoomMeta>,
}

impl Default for LocalState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            rooms: BTreeMap::new(),
        }
    }
}

/// Load the local state; a missing file is an empty default.
pub fn load(data_dir: &Path) -> CoreResult<LocalState> {
    let path = data_dir.join(STATE_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(LocalState::default()),
        Err(err) => {
            return Err(CoreError::internal(format!(
                "could not read {}: {err}",
                path.display()
            )))
        }
    };
    serde_json::from_slice(&bytes)
        .map_err(|e| CoreError::internal(format!("corrupt {}: {e}", path.display())))
}

/// Persist local state as a durable atomic replacement.
fn save(data_dir: &Path, state: &LocalState) -> CoreResult<()> {
    ensure_dir(data_dir)?;
    let path = data_dir.join(STATE_FILE);
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|e| CoreError::internal(format!("could not encode state.json: {e}")))?;
    let mut tmp = tempfile::NamedTempFile::new_in(data_dir)
        .map_err(|e| CoreError::internal(format!("could not stage {}: {e}", path.display())))?;
    tmp.write_all(&bytes)
        .and_then(|()| tmp.as_file().sync_all())
        .map_err(|e| CoreError::internal(format!("could not sync {}: {e}", path.display())))?;
    let persisted = tmp.persist(&path).map_err(|e| {
        CoreError::internal(format!("could not replace {}: {}", path.display(), e.error))
    })?;
    drop(persisted);

    // A synced file plus an unsynced rename can still disappear after sudden
    // power loss. Unix permits fsync on the containing directory. tempfile's
    // persist already uses the platform replacement primitive on Windows; std
    // does not expose a portable directory-sync operation there.
    #[cfg(unix)]
    std::fs::File::open(data_dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| {
            CoreError::internal(format!(
                "could not sync the state directory {}: {e}",
                data_dir.display()
            ))
        })?;

    Ok(())
}

fn update(data_dir: &Path, mutation: impl FnOnce(&mut LocalState)) -> CoreResult<()> {
    let _guard = STATE_WRITE_LOCK
        .lock()
        .map_err(|_| CoreError::internal("state.json write lock is poisoned"))?;
    let mut state = load(data_dir)?;
    mutation(&mut state);
    save(data_dir, &state)
}

fn room_entry<'a>(state: &'a mut LocalState, room_id: &str) -> &'a mut RoomMeta {
    state.rooms.entry(room_id.to_owned()).or_insert(RoomMeta {
        name: None,
        added_at_ms: crate::now_ms(),
        peer_hints: Vec::new(),
        fetched_files: BTreeMap::new(),
    })
}

fn merge_peer_hints(entry: &mut RoomMeta, hints: &[String]) {
    for hint in hints {
        let id_part = hint.split('@').next().unwrap_or(hint).trim().to_owned();
        entry
            .peer_hints
            .retain(|known| known.split('@').next().unwrap_or(known).trim() != id_part);
        entry.peer_hints.push(hint.trim().to_owned());
    }
}

/// Record a room in the known-rooms index (optionally with a local name).
/// Idempotent; an existing local name is kept unless `name` is `Some`.
pub fn remember_room(data_dir: &Path, room_id: &str, name: Option<&str>) -> CoreResult<()> {
    remember_room_with_peer_hints(data_dir, room_id, name, &[])
}

/// Record local room provenance, optional display name, and validated dial
/// hints in one durable state transaction.
///
/// Join uses this immediately before publishing `member.joined`: after the
/// proposed event has passed the local membership fold, but before the network
/// mutation can make the join irreversible.
pub fn remember_room_with_peer_hints(
    data_dir: &Path,
    room_id: &str,
    name: Option<&str>,
    hints: &[String],
) -> CoreResult<()> {
    update(data_dir, |state| {
        let entry = room_entry(state, room_id);
        if let Some(name) = name {
            entry.name = Some(name.to_owned());
        }
        merge_peer_hints(entry, hints);
    })
}

/// The local name override for a room, if any.
pub fn local_name(data_dir: &Path, room_id: &str) -> Option<String> {
    load(data_dir)
        .ok()
        .and_then(|s| s.rooms.get(room_id).and_then(|m| m.name.clone()))
}

/// Merge peer dial hints into a room's known set. A hint for an endpoint id
/// that already has one replaces it (a fresher address supersedes a stale
/// one — loopback ports are ephemeral per node lifetime). Idempotent.
pub fn add_peer_hints(data_dir: &Path, room_id: &str, hints: &[String]) -> CoreResult<()> {
    if hints.is_empty() {
        return Ok(());
    }
    update(data_dir, |state| {
        merge_peer_hints(room_entry(state, room_id), hints);
    })
}

/// The known peer dial hints for a room (empty when none recorded).
#[must_use]
pub fn peer_hints(data_dir: &Path, room_id: &str) -> Vec<String> {
    load(data_dir)
        .ok()
        .and_then(|s| s.rooms.get(room_id).map(|m| m.peer_hints.clone()))
        .unwrap_or_default()
}

/// Record a verified local copy produced by `file.fetch`.
pub fn remember_fetched_file(
    data_dir: &Path,
    room_id: &str,
    file_id: &str,
    path: &Path,
    bytes: u64,
) -> CoreResult<()> {
    update(data_dir, |state| {
        room_entry(state, room_id).fetched_files.insert(
            file_id.to_owned(),
            FetchedFileMeta {
                path: path.to_path_buf(),
                bytes,
                fetched_at_ms: crate::now_ms(),
            },
        );
    })
}

/// A verified local fetch record if the file still exists with the expected
/// byte length. If the user deleted or replaced the file, do not surface a stale
/// "fetched" state.
#[must_use]
pub fn fetched_file(data_dir: &Path, room_id: &str, file_id: &str) -> Option<FetchedFileMeta> {
    let meta = load(data_dir)
        .ok()?
        .rooms
        .get(room_id)?
        .fetched_files
        .get(file_id)?
        .clone();
    let ok = std::fs::metadata(&meta.path).is_ok_and(|m| m.is_file() && m.len() == meta.bytes);
    ok.then_some(meta)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::{
        fetched_file, load, local_name, remember_fetched_file, remember_room,
        remember_room_with_peer_hints,
    };
    use tempfile::tempdir;

    #[test]
    fn missing_state_is_empty_default() {
        let dir = tempdir().unwrap();
        let state = load(dir.path()).unwrap();
        assert!(state.rooms.is_empty());
    }

    #[test]
    fn remember_room_roundtrips_and_keeps_names() {
        let dir = tempdir().unwrap();
        remember_room(dir.path(), "blake3:ab", Some("Build Room")).unwrap();
        remember_room(dir.path(), "blake3:ab", None).unwrap(); // must not clobber
        remember_room(dir.path(), "blake3:cd", None).unwrap();
        assert_eq!(
            local_name(dir.path(), "blake3:ab").as_deref(),
            Some("Build Room")
        );
        assert_eq!(local_name(dir.path(), "blake3:cd"), None);
        assert_eq!(load(dir.path()).unwrap().rooms.len(), 2);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(super::STATE_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "state.json must remain owner-only");
        }
    }

    #[test]
    fn concurrent_state_mutations_do_not_lose_room_provenance() {
        const WRITERS: usize = 24;
        let dir = tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(WRITERS));
        let mut threads = Vec::with_capacity(WRITERS);
        for i in 0..WRITERS {
            let data_dir = dir.path().to_path_buf();
            let barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                let room_id = format!("blake3:{i:064x}");
                let hint = format!("{:064x}@127.0.0.1:{}", i + 1, 20_000 + i);
                barrier.wait();
                remember_room_with_peer_hints(
                    &data_dir,
                    &room_id,
                    Some(&format!("Room {i}")),
                    &[hint],
                )
                .unwrap();
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }

        let state = load(dir.path()).unwrap();
        assert_eq!(state.rooms.len(), WRITERS);
        assert!(state.rooms.values().all(|room| room.peer_hints.len() == 1));
    }

    #[test]
    fn fetched_file_roundtrips_only_while_file_exists_with_expected_size() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("download.txt");
        std::fs::write(&file, b"hello").unwrap();
        remember_fetched_file(dir.path(), "blake3:ab", "file_01", &file, 5).unwrap();

        let fetched = fetched_file(dir.path(), "blake3:ab", "file_01").unwrap();
        assert_eq!(fetched.path, file);
        assert_eq!(fetched.bytes, 5);

        std::fs::write(&file, b"changed").unwrap();
        assert!(fetched_file(dir.path(), "blake3:ab", "file_01").is_none());
    }
}
