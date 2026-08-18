//! The monotonic `Instant` → logical `Tick` mapping for the native driver.
//!
//! The sans-IO core takes time as an input ([`Tick`]) and never reads a wall
//! clock. The native driver realizes logical time from a monotonic
//! [`Instant`](std::time::Instant) captured once at construction, at **1 tick =
//! 1 ms** — matching the [`KernelLimits`](crate::KernelLimits) default comment
//! (30 000 ticks ⇒ a 30-second deadline). This file lives under `src/adapter/`,
//! outside `src/kernel/**`, so its `std::time` use is legitimate and the
//! sans-IO boundary scan stays green.

use std::time::Instant;

use crate::kernel::timing::Tick;

/// A monotonic logical clock. Cloneable base only; every read is a fresh
/// `Instant::now()` diff against the captured base.
pub(crate) struct Clock {
    base: Instant,
}

impl Clock {
    /// Capture the origin instant. Logical time zero is this moment.
    pub(crate) fn new() -> Self {
        Self {
            base: Instant::now(),
        }
    }

    /// The current logical time: milliseconds elapsed since `base`, saturating
    /// at [`u64::MAX`] rather than wrapping (mirrors `Tick::saturating_add`'s
    /// honest-failure direction — time never silently jumps backward).
    pub(crate) fn now(&self) -> Tick {
        let millis = self.base.elapsed().as_millis();
        Tick::from_millis(millis.min(u128::from(u64::MAX)) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_monotonic_nondecreasing() {
        let clock = Clock::new();
        let first = clock.now();
        let second = clock.now();
        assert!(second >= first, "logical time never runs backward");
    }

    #[test]
    fn tick_resolution_approximates_one_ms_per_tick() {
        let clock = Clock::new();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let after = clock.now();
        // 1 tick = 1 ms; after sleeping ≥20 ms, we must see at least 5 ticks.
        // The lower bound is conservative to tolerate slow or heavily loaded VMs.
        assert!(
            after >= Tick::from_millis(5),
            "after a 20 ms sleep, clock must advance at least 5 ticks (got {after:?})"
        );
    }
}
