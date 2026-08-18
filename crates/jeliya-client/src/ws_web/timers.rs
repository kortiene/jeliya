//! The platform clock and one-shot timers (§6.5).
//!
//! `now` reads `performance.now()` (monotonic milliseconds) mapped to a kernel
//! [`Tick`] (one tick = one ms). Kernel timers ride `window.setTimeout` /
//! `clearTimeout`. No `std::time`, no `Date.now`, is ever read inside the
//! library. `jitter_seed` entropy comes from `crypto.getRandomValues`.

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::kernel::timing::Tick;

/// The browser `Window`, which always exists in the adapter's context.
fn window() -> Option<web_sys::Window> {
    web_sys::window()
}

/// The current monotonic time as a kernel [`Tick`] (one tick = one ms).
pub(crate) fn now_tick() -> Tick {
    let ms = window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0);
    // `performance.now()` is a non-negative monotonic float; clamp to the
    // logical-tick integer domain.
    Tick(ms.max(0.0) as u64)
}

/// One `u64` of platform entropy for the reconnect jitter PRNG. Not a
/// credential — it only decorrelates reconnect storms (§6.8).
pub(crate) fn random_u64() -> u64 {
    let mut buf = [0u8; 8];
    if let Some(win) = window() {
        if let Ok(crypto) = win.crypto() {
            let _ = crypto.get_random_values_with_u8_array(&mut buf);
        }
    }
    u64::from_le_bytes(buf)
}

/// Schedule `cb` to fire once after `delay_ms`, returning the browser timer
/// handle for later cancellation.
pub(crate) fn set_timeout(delay_ms: i32, cb: &Closure<dyn FnMut()>) -> Option<i32> {
    window().and_then(|w| {
        w.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            delay_ms,
        )
        .ok()
    })
}

/// Cancel a previously-scheduled timer by its browser handle.
pub(crate) fn clear_timeout(handle: i32) {
    if let Some(w) = window() {
        w.clear_timeout_with_handle(handle);
    }
}
