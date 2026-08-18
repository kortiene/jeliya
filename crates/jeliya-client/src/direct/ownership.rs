//! One live owner per canonical data directory (§5.4).
//!
//! DirectClient hosts the engine in-process; two `DirectClient`s over the same
//! data dir would open two `Engine`s against one `rooms.db`, racing the WAL
//! transition the serialized dispatch task exists to prevent. This process-global
//! registry makes that unrepresentable: the first `connect_direct` for a dir
//! acquires exclusive ownership, and a second **fails closed**
//! ([`OwnershipError::AlreadyOwned`]) rather than opening a second engine. The
//! grant is an [`OwnerToken`] whose `Drop` releases the entry, so teardown (or a
//! dropped actor) frees the dir for a later `connect_direct` — supporting the
//! repeated-start/stop acceptance criterion.
//!
//! **Scope (OQ-2).** This is a *process*-global guard, matching `FfiHost`'s
//! process-singleton `HOST`; it does not protect against a second OS process
//! opening the same on-disk dir. On Android the app is a single process, and the
//! engine's own per-room `rooms.db`/blob locks are the cross-process backstop.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// Why a data directory could not be acquired for a new `DirectClient`.
///
/// The direct path surfaces this as a terminal `Failed` state (a gate refusal
/// carries no auto-retry — there is nothing to back off to), never as a socket
/// error: DirectClient has no socket.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
pub enum OwnershipError {
    /// A live owner already controls this canonical data directory. Opening a
    /// second engine over it would race the first's storage locks, so the
    /// request is refused rather than corrupting state.
    #[error("data directory is already owned by a live DirectClient in this process")]
    AlreadyOwned,
}

/// An exclusive grant for one canonical data directory. Held by the live
/// [`DirectEngineActor`](super::actor::DirectEngineActor) for its whole life;
/// dropping it (teardown, or a dropped actor) releases the directory.
///
/// The token holds the registry key and a strong reference to the per-dir slot;
/// the registry keeps only a [`Weak`], so the slot — and thus ownership — lives
/// exactly as long as the token.
pub(crate) struct OwnerToken {
    dir: PathBuf,
    _slot: Arc<()>,
}

impl Drop for OwnerToken {
    fn drop(&mut self) {
        registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.dir);
    }
}

/// The process-global directory→owner map. Entries are [`Weak`], so a slot
/// vanishes the instant its [`OwnerToken`] drops even if `acquire` never runs
/// again; `acquire` additionally prunes any dead entry it finds, keeping the map
/// bounded under start/stop churn.
fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<()>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<()>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Canonicalize a data dir the same way `Engine::new`/`FfiHost::same_data_dir`
/// do — ensure it exists, then canonicalize, falling back to the spelled path —
/// so "the same dir spelled differently" resolves to one owner.
fn canonicalize(dir: &Path) -> PathBuf {
    let _ = jeliya_core::identity::ensure_dir(dir);
    dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf())
}

/// Acquire exclusive ownership of `dir`'s canonical form, or fail closed if a
/// live owner already holds it. A dead weak entry (its token already dropped) is
/// pruned and the dir re-acquired, so a clean teardown always frees the dir.
pub(crate) fn acquire(dir: &Path) -> Result<OwnerToken, OwnershipError> {
    let canonical = canonicalize(dir);
    let mut map = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = map.get(&canonical) {
        if existing.strong_count() > 0 {
            return Err(OwnershipError::AlreadyOwned);
        }
        // A stale weak entry from a released owner: prune and re-acquire.
        map.remove(&canonical);
    }
    let slot = Arc::new(());
    map.insert(canonical.clone(), Arc::downgrade(&slot));
    Ok(OwnerToken {
        dir: canonical,
        _slot: slot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_owner_of_the_same_dir_fails_closed_and_release_frees_it() {
        let temp = std::env::temp_dir().join(format!("jeliya-direct-own-{}", std::process::id()));
        let first = acquire(&temp).expect("first owner");
        assert!(
            matches!(acquire(&temp), Err(OwnershipError::AlreadyOwned)),
            "a live owner blocks a second"
        );
        drop(first);
        let third = acquire(&temp).expect("released dir is re-acquirable");
        drop(third);
        let _ = std::fs::remove_dir_all(&temp);
    }
}
