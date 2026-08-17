//! Logical monotonic time (§K8) — **no wall clock**.
//!
//! The sans-IO core takes time as an *input* ([`Tick`]) and emits "arm a timer
//! at `Tick`" as an *action*; it never reads `std::time`. The deterministic
//! test driver owns a virtual clock and advances it explicitly; real adapters
//! own the platform clock/timer, outside this library. `boundaries.rs` asserts
//! that `std::time`, `Instant::now`, and `SystemTime` appear in no kernel
//! source file, so this discipline is enforced, not merely intended.

/// A logical, monotonic instant. Its unit is deliberately abstract: a host maps
/// one `Tick` to a real duration when it drives the kernel, but the kernel and
/// its tests only ever compare and add ticks, so behaviour is identical on
/// wasm and native.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub(crate) struct Tick(pub(crate) u64);

/// A span of logical ticks (a deadline offset or a backoff delay).
///
/// This is the one timing type that appears in the crate's public surface —
/// the deltas of [`KernelLimits`](crate::KernelLimits). A host builds one with
/// [`TickDelta::from_ticks`] and reads it back with [`TickDelta::ticks`]; the
/// unit is deliberately abstract (a logical tick, not a fixed duration — the
/// host maps one tick to a real duration when it drives the kernel).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct TickDelta(pub(crate) u64);

impl TickDelta {
    /// A delta of `ticks` logical ticks.
    pub const fn from_ticks(ticks: u64) -> Self {
        TickDelta(ticks)
    }

    /// The number of logical ticks this delta spans.
    pub const fn ticks(self) -> u64 {
        self.0
    }
}

impl Tick {
    /// The origin instant. A fresh virtual clock starts here.
    pub(crate) const ZERO: Tick = Tick(0);

    /// Build a tick from a millisecond count. A native adapter maps its
    /// monotonic clock to logical time at **1 tick = 1 ms** (matching the
    /// [`KernelLimits`](crate::KernelLimits) default comment); the sans-IO core
    /// never calls this — it only ever receives a `Tick` as an input.
    pub(crate) const fn from_millis(ms: u64) -> Tick {
        Tick(ms)
    }

    /// The instant `delta` after `self`, saturating at [`u64::MAX`] rather than
    /// wrapping — a non-representable deadline never silently becomes an
    /// *earlier* one (mirrors the protocol's "not representable by a finite
    /// timer … refuses readiness": saturation is the honest failure direction).
    pub(crate) fn saturating_add(self, delta: TickDelta) -> Tick {
        Tick(self.0.saturating_add(delta.0))
    }

    /// Advance in place by `delta` (the virtual clock's step).
    pub(crate) fn advance(&mut self, delta: TickDelta) {
        self.0 = self.0.saturating_add(delta.0);
    }

    /// The milliseconds from `earlier` to `self` (1 tick = 1 ms), saturating at
    /// zero when `self` is not after `earlier`. A native driver uses this to
    /// turn an absolute deadline `Tick` into a `sleep` delay.
    pub(crate) fn saturating_ms_from(self, earlier: Tick) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

/// A driver-scheduled timer's identity. The core allocates these monotonically;
/// the number of live timers is bounded by the in-flight cap plus the single
/// reconnect timer (§K12), so the space never grows without bound.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub(crate) struct TimerId(pub(crate) u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturating_add_never_wraps_past_max() {
        let near = Tick(u64::MAX - 2);
        assert_eq!(near.saturating_add(TickDelta(10)), Tick(u64::MAX));
    }

    #[test]
    fn add_is_ordinary_below_the_ceiling() {
        assert_eq!(Tick(5).saturating_add(TickDelta(7)), Tick(12));
    }

    #[test]
    fn advance_moves_the_clock_forward() {
        let mut now = Tick::ZERO;
        now.advance(TickDelta(3));
        now.advance(TickDelta(4));
        assert_eq!(now, Tick(7));
    }

    #[test]
    fn ticks_order_as_their_underlying_values() {
        assert!(Tick(1) < Tick(2));
        assert!(TimerId(1) < TimerId(2));
    }
}
