//! Plural category selection per locale (§5.2).
//!
//! A plural message never inlines `if count == 1` in the catalog: the *rule*
//! differs by language, and hard-coding English's rule into a French sentence
//! is exactly the drift the two-catalog design exists to prevent. The caller
//! (the [`crate::l10n::Formats`] seam, or a component that already holds the
//! text [`Locale`]) computes the [`PluralCategory`] from the raw count under the
//! **text** locale's rules, formats the number under the **formatting** locale,
//! and hands both to the catalog method — so the number's grouping and the
//! word's plural form are chosen independently, exactly as they must be.
//!
//! Only the two CLDR categories the supported locales use are modelled. English
//! and French both collapse to `one`/`other`; adding a locale with `few`/`many`
//! (Polish, Arabic) extends this enum and every plural catalog method in
//! lockstep, which is the point of making the category an explicit type rather
//! than a boolean.

use super::Locale;

/// The plural category a count selects, for the locales this crate supports.
///
/// A closed set matching the CLDR categories English and French use. A catalog
/// plural method matches on [`PluralCategory::One`] and treats every other arm
/// as [`PluralCategory::Other`], so a future `few`/`many` category is a compile
/// error at every plural site until it is handled — never a silently-wrong
/// fallthrough.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PluralCategory {
    /// The singular category. English: `n == 1`. French: `n == 0 || n == 1`.
    One,
    /// Everything else.
    Other,
}

/// The plural category `count` selects under `locale`'s rules.
///
/// - English: `1 → one`, else `other` (so `0 → other`: "0 rooms").
/// - French: `0` and `1 → one`, else `other` (so `0 → one`: "0 salon"),
///   matching CLDR and the French Flutter/React catalogs.
pub fn category(locale: Locale, count: u64) -> PluralCategory {
    match locale {
        Locale::En => {
            if count == 1 {
                PluralCategory::One
            } else {
                PluralCategory::Other
            }
        }
        Locale::Fr => {
            if count == 0 || count == 1 {
                PluralCategory::One
            } else {
                PluralCategory::Other
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_singular_is_one_only() {
        assert_eq!(category(Locale::En, 0), PluralCategory::Other);
        assert_eq!(category(Locale::En, 1), PluralCategory::One);
        assert_eq!(category(Locale::En, 2), PluralCategory::Other);
        assert_eq!(category(Locale::En, 5), PluralCategory::Other);
    }

    #[test]
    fn french_zero_and_one_are_singular() {
        assert_eq!(category(Locale::Fr, 0), PluralCategory::One);
        assert_eq!(category(Locale::Fr, 1), PluralCategory::One);
        assert_eq!(category(Locale::Fr, 2), PluralCategory::Other);
        assert_eq!(category(Locale::Fr, 5), PluralCategory::Other);
    }
}
