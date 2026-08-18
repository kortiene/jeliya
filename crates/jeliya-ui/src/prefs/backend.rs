//! The raw key/value backend the preference schema layers over (#178 §3.2).
//!
//! [`KeyValueBackend`] is a deliberately tiny string→string store: `get` /
//! `set` / `remove` / `keys`. The schema layer ([`super::PreferenceSchema`])
//! owns the namespace, the version envelope, the recovery policy, and the typed
//! [`jeliya_platform::PreferenceKey`] derivation; the backend knows none of
//! that, so the *decisions* stay host-testable and the browser binding
//! (`jeliya-platform-web`) supplies only the storage.
//!
//! The shipped browser backend is [`InMemoryBackend`] — nothing survives the
//! tab (D3). It is also the exact fixture the corrupt/unsupported-version
//! recovery tests seed, and the shape the persisting platforms (#185/#173) will
//! back with real durable storage behind the same trait.

use std::cell::RefCell;
use std::collections::BTreeMap;

/// A raw device-local string store. Single-threaded (`&self`, interior
/// mutability): the browser target is single-threaded, matching the rest of the
/// UI's `Rc`/`RefCell` discipline.
pub trait KeyValueBackend {
    /// Read the raw value at `key`, or `None` if unset.
    fn get(&self, key: &str) -> Option<String>;

    /// Write the raw `value` at `key`.
    fn set(&self, key: &str, value: &str);

    /// Remove `key`. A no-op if it was unset.
    fn remove(&self, key: &str);

    /// Every key currently present, in unspecified order. Used only to clear a
    /// whole namespace on a full reset (§6.4); the schema never enumerates keys
    /// to *read* them.
    fn keys(&self) -> Vec<String>;
}

/// An in-memory [`KeyValueBackend`] — the shipped browser store (D3) and the
/// test fixture. Never `localStorage` / `sessionStorage`: a reload is a fresh
/// session by construction, so nothing wrongly restores.
#[derive(Default)]
pub struct InMemoryBackend {
    map: RefCell<BTreeMap<String, String>>,
}

impl InMemoryBackend {
    /// A fresh, empty backend — the production browser start state each tab.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a backend with raw entries, for the recovery/corruption fixtures
    /// (corrupt bytes, a future-version envelope, a seeded `LastRoom`). Test and
    /// fixture support only; production always starts empty.
    pub fn seeded(entries: impl IntoIterator<Item = (String, String)>) -> Self {
        let backend = Self::new();
        for (key, value) in entries {
            backend.set(&key, &value);
        }
        backend
    }
}

impl KeyValueBackend for InMemoryBackend {
    fn get(&self, key: &str) -> Option<String> {
        self.map.borrow().get(key).cloned()
    }

    fn set(&self, key: &str, value: &str) {
        self.map
            .borrow_mut()
            .insert(key.to_owned(), value.to_owned());
    }

    fn remove(&self, key: &str) {
        self.map.borrow_mut().remove(key);
    }

    fn keys(&self) -> Vec<String> {
        self.map.borrow().keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_backend_round_trips_and_removes() {
        let backend = InMemoryBackend::new();
        assert_eq!(backend.get("k"), None);
        backend.set("k", "v");
        assert_eq!(backend.get("k"), Some("v".to_owned()));
        backend.set("k", "v2");
        assert_eq!(backend.get("k"), Some("v2".to_owned()));
        backend.remove("k");
        assert_eq!(backend.get("k"), None);
        // Removing an absent key is a no-op.
        backend.remove("absent");
    }

    #[test]
    fn seeded_backend_exposes_its_keys() {
        let backend = InMemoryBackend::seeded([
            ("a".to_owned(), "1".to_owned()),
            ("b".to_owned(), "2".to_owned()),
        ]);
        let mut keys = backend.keys();
        keys.sort();
        assert_eq!(keys, vec!["a".to_owned(), "b".to_owned()]);
    }
}
