//! Responsive shell selection (#178 §8), the one Rust breakpoint fork the
//! shared shell needs.
//!
//! CSS owns the *layout* — which panes show and where — in the single canonical
//! `ui/src/styles.css` (consumed byte-identically per #176/#177). The **one**
//! difference CSS cannot make by hiding an element is the compact
//! global-destination bottom bar vs. the wide/medium rail/header: rendering both
//! and hiding one would put two nav landmarks (and two room titles) in the
//! accessibility tree. So the shell selects the *element* from an injected
//! [`Shell`] value (Decision-6 / D6), never a `cfg`, and this module holds the
//! width→shell decision plus the breakpoint constants that must agree with the
//! stylesheet's media queries (asserted in the tests phase against
//! `ui/src/styles.css`).
//!
//! This module is pure and host-testable: `web-sys`'s `matchMedia` subscription
//! lives in `jeliya-platform-web`/`compose`, which pushes the resolved [`Shell`]
//! in as a reactive input.

pub mod bootstrap;
pub mod router;

/// The largest viewport width, in CSS pixels, that is still the **compact**
/// shell. Fractional (`899.98`) so a scaled or zoomed fractional width cannot
/// fall *between* the compact `max-width: 899.98px` media query and the medium
/// `min-width: 900px` one and match neither — the retiring `shell.ts`
/// invariant, preserved verbatim.
pub const COMPACT_MAX: f64 = 899.98;

/// The smallest viewport width, in CSS pixels, that is the **medium** shell.
pub const MEDIUM_MIN: f64 = 900.0;

/// The largest viewport width, in CSS pixels, that is still the **medium**
/// shell (the wide shell begins at [`WIDE_MIN`]). Fractional for the same
/// no-gap reason as [`COMPACT_MAX`].
pub const MEDIUM_MAX: f64 = 1279.98;

/// The smallest viewport width, in CSS pixels, that is the **wide** shell.
pub const WIDE_MIN: f64 = 1280.0;

/// The compact-shell media query, mirroring the stylesheet's
/// `@media (max-width: 899.98px)`. The parity test asserts this string appears
/// verbatim in `ui/src/styles.css`, so the constant and the CSS can never drift.
pub const COMPACT_MEDIA_QUERY: &str = "(max-width: 899.98px)";

/// The medium-shell media query, mirroring
/// `@media (max-width: 1279.98px) and (min-width: 900px)`.
pub const MEDIUM_MEDIA_QUERY: &str = "(max-width: 1279.98px) and (min-width: 900px)";

/// The wide-shell media query, mirroring `@media (min-width: 1280px)`.
pub const WIDE_MEDIA_QUERY: &str = "(min-width: 1280px)";

/// Which responsive shell a viewport width selects (#58 / contract §"Responsive
/// shells and viewports").
///
/// The nav *topology* each shell carries:
///
/// - [`Shell::Compact`] — one pane at a time; global destinations in a bottom
///   bar carrying **only** global destinations; room destinations via nested
///   navigation inside Rooms.
/// - [`Shell::Medium`] — room rail + workspace; the room-destination inspector
///   opens as a dismissible drawer.
/// - [`Shell::Wide`] — room rail + workspace + inspector as a third column.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Shell {
    /// Phones and narrow windows (`< 900px`).
    Compact,
    /// Tablets and split windows (`900–1279px`).
    Medium,
    /// Desktops and wide windows (`>= 1280px`).
    Wide,
}

impl Shell {
    /// Whether this shell places the global-destination navigation in a compact
    /// **bottom bar** (as opposed to the wide/medium rail/header). The single
    /// element-identity fork the shell drives from an injected [`Shell`].
    pub fn uses_bottom_bar(self) -> bool {
        matches!(self, Shell::Compact)
    }

    /// Whether this shell shows the room-destination inspector as a **persistent
    /// third column** (wide) rather than a dismissible drawer (medium) or nested
    /// navigation (compact).
    pub fn inspector_is_column(self) -> bool {
        matches!(self, Shell::Wide)
    }
}

/// The shell a viewport `width` (CSS pixels) selects.
///
/// The boundaries match the stylesheet exactly: `< 900` is compact (the CSS
/// `max-width: 899.98px`), `900..1280` is medium, `>= 1280` is wide. A width
/// landing on the fractional gap (e.g. `899.99`) resolves to the lower shell
/// deterministically — it is below `MEDIUM_MIN`, so it is compact — which is why
/// the CSS uses the fractional `899.98` upper bound rather than `900`.
pub fn shell_for(width: f64) -> Shell {
    if width >= WIDE_MIN {
        Shell::Wide
    } else if width >= MEDIUM_MIN {
        Shell::Medium
    } else {
        Shell::Compact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_for_selects_at_and_around_every_breakpoint() {
        // Compact below 900, including the fractional gap the CSS bound closes.
        assert_eq!(shell_for(360.0), Shell::Compact);
        assert_eq!(shell_for(899.0), Shell::Compact);
        assert_eq!(shell_for(COMPACT_MAX), Shell::Compact); // 899.98
        assert_eq!(shell_for(899.99), Shell::Compact);
        // Medium from exactly 900 through the fractional upper bound.
        assert_eq!(shell_for(MEDIUM_MIN), Shell::Medium); // 900
        assert_eq!(shell_for(920.0), Shell::Medium);
        assert_eq!(shell_for(MEDIUM_MAX), Shell::Medium); // 1279.98
        assert_eq!(shell_for(1279.0), Shell::Medium);
        // Wide from exactly 1280.
        assert_eq!(shell_for(WIDE_MIN), Shell::Wide); // 1280
        assert_eq!(shell_for(1920.0), Shell::Wide);
    }

    // The breakpoint constants are ordered with no gap: COMPACT_MAX is strictly
    // below MEDIUM_MIN and MEDIUM_MAX strictly below WIDE_MIN. Pinned as
    // compile-time assertions so a constant edit that opened a gap would not
    // even build.
    const _: () = assert!(COMPACT_MAX < MEDIUM_MIN);
    const _: () = assert!(MEDIUM_MAX < WIDE_MIN);

    #[test]
    fn the_constants_leave_no_gap_between_compact_and_medium() {
        // Every width in the fractional gap resolves down to compact (never
        // "neither"); the ordering itself is the compile-time assertions above.
        for width in [COMPACT_MAX, 899.985, 899.999] {
            assert_eq!(shell_for(width), Shell::Compact, "gap width {width}");
        }
    }

    #[test]
    fn element_identity_forks_track_the_shell() {
        assert!(Shell::Compact.uses_bottom_bar());
        assert!(!Shell::Medium.uses_bottom_bar());
        assert!(!Shell::Wide.uses_bottom_bar());
        assert!(Shell::Wide.inspector_is_column());
        assert!(!Shell::Medium.inspector_is_column());
        assert!(!Shell::Compact.inspector_is_column());
    }
}
