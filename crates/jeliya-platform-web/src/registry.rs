//! The fail-closed minted-token registry (#174 §K4, #181 D0).
//!
//! Anti-forgery does not rest on the compile-time absence of the factory door
//! alone — under Cargo feature unification the factory module is compiled into
//! the one `jeliya-platform` instance a target binary links (tier 2). What holds
//! regardless is this **runtime gate**: a handle a service did not mint resolves
//! nowhere. Every [`Registry`] carries a unique **issuer** stamped into the high
//! bits of each token it mints; [`Registry::resolve`] rejects a token whose
//! issuer is not its own, so a foreign or forged token — even another registry's
//! genuine token — fails closed.
//!
//! The registry is portable (no `web-sys`), so it is unit-tested on the host;
//! the browser bindings key it with `web-sys` values (a `File`, an in-memory
//! blob) on `wasm32`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// A process-global issuer counter, so two registries in one binary never share
/// an issuer and thus never resolve each other's tokens.
static NEXT_ISSUER: AtomicU32 = AtomicU32::new(1);

/// A minted-token table keyed by an issuer-stamped `u64`.
///
/// The token layout is `(issuer as u64) << 32 | local_id`. `resolve`/`take`/
/// `remove` fail closed on any token whose top 32 bits are not this registry's
/// issuer — the runtime half of the anti-forgery guarantee (§K4).
pub struct Registry<T> {
    issuer: u32,
    next_local: Cell<u32>,
    entries: RefCell<HashMap<u64, T>>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Registry<T> {
    /// A fresh registry with a unique issuer.
    pub fn new() -> Self {
        Self {
            issuer: NEXT_ISSUER.fetch_add(1, Ordering::Relaxed),
            next_local: Cell::new(1),
            entries: RefCell::new(HashMap::new()),
        }
    }

    /// Mint a token for `value`, returning the issuer-stamped raw `u64` the
    /// caller wraps through the factory door.
    pub fn mint(&self, value: T) -> u64 {
        let local = self.next_local.get();
        self.next_local.set(local.wrapping_add(1));
        let token = (u64::from(self.issuer) << 32) | u64::from(local);
        self.entries.borrow_mut().insert(token, value);
        token
    }

    /// Whether `token` was minted by this registry (issuer match) and is still
    /// present. A foreign issuer fails this before any lookup (§K4).
    pub fn contains(&self, token: u64) -> bool {
        self.issued_here(token) && self.entries.borrow().contains_key(&token)
    }

    /// Run `f` over the value behind `token`, or `None` if the token was not
    /// minted here or is absent. Borrow-not-consume: a failed operation leaves
    /// the entry recoverable (#174 D5).
    pub fn with<R>(&self, token: u64, f: impl FnOnce(&T) -> R) -> Option<R> {
        if !self.issued_here(token) {
            return None;
        }
        self.entries.borrow().get(&token).map(f)
    }

    /// Run `f` over a mutable reference to the value behind `token`.
    pub fn with_mut<R>(&self, token: u64, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        if !self.issued_here(token) {
            return None;
        }
        self.entries.borrow_mut().get_mut(&token).map(f)
    }

    /// Remove and return the value behind `token`, or `None` if it was not
    /// minted here or is absent.
    pub fn take(&self, token: u64) -> Option<T> {
        if !self.issued_here(token) {
            return None;
        }
        self.entries.borrow_mut().remove(&token)
    }

    /// Drop the entry behind `token`. Returns whether an entry was removed —
    /// `false` for a foreign or already-released token, so a release is
    /// idempotent and fails closed.
    pub fn remove(&self, token: u64) -> bool {
        self.take(token).is_some()
    }

    /// Whether `token`'s issuer is this registry's.
    fn issued_here(&self, token: u64) -> bool {
        (token >> 32) as u32 == self.issuer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_token_resolves_here() {
        let reg = Registry::<String>::new();
        let token = reg.mint("bytes".to_string());
        assert!(reg.contains(token));
        assert_eq!(reg.with(token, |v| v.clone()), Some("bytes".to_string()));
    }

    #[test]
    fn a_foreign_token_fails_closed() {
        let a = Registry::<String>::new();
        let b = Registry::<String>::new();
        let token = a.mint("secret".to_string());
        // Another registry's genuine token resolves nowhere in `b` (§K4).
        assert!(!b.contains(token));
        assert_eq!(b.with(token, |v| v.clone()), None);
        assert_eq!(b.take(token), None);
        assert!(!b.remove(token));
        // A never-minted token also fails closed.
        assert!(!a.contains(0xDEAD_BEEF_0000_0001));
    }

    #[test]
    fn take_and_remove_are_idempotent() {
        let reg = Registry::<u32>::new();
        let token = reg.mint(7);
        assert_eq!(reg.take(token), Some(7));
        // A second take/remove is a fail-closed no-op, not a panic.
        assert_eq!(reg.take(token), None);
        assert!(!reg.remove(token));
    }

    #[test]
    fn with_mut_modifies_entry_in_place() {
        let reg = Registry::<Vec<u8>>::new();
        let token = reg.mint(vec![1, 2]);
        assert_eq!(reg.with(token, |v| v.len()), Some(2));
        reg.with_mut(token, |v| v.push(3));
        assert_eq!(reg.with(token, |v| v.len()), Some(3));
        // A foreign-issuer token must not resolve (§K4).
        let other: Registry<Vec<u8>> = Registry::new();
        assert!(other.with_mut(token, |v| v.push(0)).is_none());
    }

    #[test]
    fn multiple_entries_track_independently() {
        let reg = Registry::<u32>::new();
        let a = reg.mint(10);
        let b = reg.mint(20);
        assert_ne!(a, b, "two mints produce distinct tokens");
        assert_eq!(reg.with(a, |v| *v), Some(10));
        assert_eq!(reg.with(b, |v| *v), Some(20));
        // Removing one entry leaves the other intact.
        reg.remove(a);
        assert!(!reg.contains(a));
        assert!(reg.contains(b));
    }
}
