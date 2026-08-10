//! Capped, full-jitter exponential backoff over a deterministic in-core PRNG
//! (§K10).
//!
//! Reconnect attempt `n` waits a random duration in
//! `[0, min(cap, base * 2^n))`. The randomness is a **deterministic in-core
//! xorshift PRNG** seeded from [`KernelConfig::jitter_seed`], never
//! `rand`/`getrandom` (wasm-hostile and non-reproducible), so the backoff
//! sequence for a fixed seed is exact in tests and identical on wasm and
//! native. Exhaustion after `max_attempts` is honest: the core transitions to
//! `Failed` and settles every outstanding call, rather than spinning forever.
//!
//! [`KernelConfig::jitter_seed`]: crate::KernelConfig

use crate::kernel::timing::TickDelta;

/// A minimal xorshift64 PRNG. Not cryptographic — it only decorrelates
/// reconnect storms — but fully deterministic given its seed, which is what the
/// no-`getrandom` boundary requires.
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    /// Seed the generator. A zero seed would make xorshift a fixed point, so it
    /// is nudged to a non-zero constant (the seed is not a credential; this
    /// only keeps the sequence live).
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// The next 64-bit value, advancing the state.
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
}

/// Full-jitter backoff with a hard attempt ceiling.
pub(crate) struct Backoff {
    base: u64,
    cap: u64,
    prng: XorShift64,
}

impl Backoff {
    /// Build the schedule from the base/cap deltas and the deterministic seed.
    pub(crate) fn new(base: TickDelta, cap: TickDelta, seed: u64) -> Self {
        Self {
            base: base.0,
            cap: cap.0,
            prng: XorShift64::new(seed),
        }
    }

    /// The delay before reconnect attempt `attempt` (0-based): a uniform draw in
    /// `[0, window)` where `window = min(cap, base * 2^attempt)`. The exponent
    /// uses a saturating shift so a large `attempt` clamps to `cap` rather than
    /// overflowing. Advances the PRNG, so repeated calls form the seeded
    /// sequence.
    pub(crate) fn delay(&mut self, attempt: u32) -> TickDelta {
        let scaled = self.base.checked_shl(attempt).unwrap_or(u64::MAX);
        let window = scaled.min(self.cap);
        if window == 0 {
            return TickDelta(0);
        }
        TickDelta(self.prng.next_u64() % window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_seed_gives_a_reproducible_sequence() {
        let mut a = Backoff::new(TickDelta(100), TickDelta(10_000), 42);
        let mut b = Backoff::new(TickDelta(100), TickDelta(10_000), 42);
        for attempt in 0..8 {
            assert_eq!(a.delay(attempt), b.delay(attempt), "attempt {attempt}");
        }
    }

    #[test]
    fn delay_never_exceeds_the_window() {
        let base = 100u64;
        let cap = 10_000u64;
        let mut backoff = Backoff::new(TickDelta(base), TickDelta(cap), 7);
        for attempt in 0..12 {
            let window = base.checked_shl(attempt).unwrap_or(u64::MAX).min(cap);
            let delay = backoff.delay(attempt).0;
            assert!(
                delay < window.max(1),
                "attempt {attempt}: delay {delay} not < window {window}"
            );
        }
    }

    #[test]
    fn the_exponential_window_saturates_at_cap_without_overflow() {
        // A huge attempt count must not panic on shift overflow.
        let mut backoff = Backoff::new(TickDelta(1), TickDelta(64), 1);
        let delay = backoff.delay(200).0;
        assert!(delay < 64);
    }

    #[test]
    fn a_zero_window_yields_zero_delay() {
        let mut backoff = Backoff::new(TickDelta(0), TickDelta(0), 1);
        assert_eq!(backoff.delay(0), TickDelta(0));
        assert_eq!(backoff.delay(5), TickDelta(0));
    }

    #[test]
    fn a_zero_seed_still_produces_a_live_sequence() {
        let mut backoff = Backoff::new(TickDelta(1000), TickDelta(1_000_000), 0);
        // Not all draws collapse to a single value.
        let draws: Vec<u64> = (0..5).map(|n| backoff.delay(n).0).collect();
        assert!(draws.iter().any(|&d| d != draws[0]));
    }
}
