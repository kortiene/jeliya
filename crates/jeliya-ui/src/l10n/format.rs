//! The single formatting seam (§4-D4, §5.3). Nothing else in the crate may
//! format a number, a percentage, or a byte size.
//!
//! Two locales, split in two (i18n.md rule 4): **vocabulary** (byte-unit words,
//! Today/Yesterday) follows the **text** locale; **conventions** (decimal and
//! group separators, percent spacing) follow the **formatting** locale. So a
//! French reader on an English-formatting device sees `octets` with `1,234`,
//! and an English reader on a French device sees `bytes` with `1 234` — each
//! fact from the locale that owns it.
//!
//! No `Intl`, no `icu4x`. `Intl` is unavailable to portable Rust and would force
//! a `web-sys`/`cfg` fork (forbidden in shared components); a heavy CLDR
//! dependency is deferred until a product surface needs an arbitrary third
//! formatting locale (spec §14 Q2). For the two supported conventions a small
//! table is exact and dependency-free. The seam accepts any tag upstream
//! (resolved to a supported convention in [`super::LocaleState::resolve`]), so
//! widening later changes this table, not the call sites.

use super::{catalog_for, Catalog, Locale};

/// Locale-aware number/percent/byte formatting bound to a (text, formatting)
/// pair.
///
/// Cheap to construct per render (two enum copies), which is what lets
/// [`super::use_formats`] re-resolve on every locale switch.
#[derive(Clone, Copy)]
pub struct Formats {
    text: Locale,
    formatting: Locale,
}

/// The decimal and group separators for a formatting convention.
struct NumberConventions {
    decimal: &'static str,
    group: &'static str,
}

impl Formats {
    /// Bind the seam to a text locale (vocabulary) and a formatting locale
    /// (numeric conventions). Deliberately two arguments: they are independent.
    pub fn new(text: Locale, formatting: Locale) -> Self {
        Self { text, formatting }
    }

    /// The formatting locale currently in effect (numeric conventions).
    pub fn formatting_locale(self) -> Locale {
        self.formatting
    }

    fn strings(self) -> &'static dyn Catalog {
        catalog_for(self.text)
    }

    fn conventions(self) -> NumberConventions {
        match self.formatting {
            // English: 1,234.56
            Locale::En => NumberConventions {
                decimal: ".",
                group: ",",
            },
            // French: 1 234,56 — the group separator is a NARROW NO-BREAK SPACE
            // (U+202F), the same character the French typography gate demands
            // before `%`. A plain space here would let a number wrap mid-group.
            Locale::Fr => NumberConventions {
                decimal: ",",
                group: "\u{202f}",
            },
        }
    }

    /// A whole number under the formatting locale's grouping (`1 234` / `1,234`).
    pub fn count(self, n: u64) -> String {
        group_digits(&n.to_string(), self.conventions().group)
    }

    /// A number with a fixed number of fractional digits under the formatting
    /// locale's separators (`1 234,56` / `1,234.56`).
    pub fn decimal(self, value: f64, frac_digits: usize) -> String {
        let conventions = self.conventions();
        // `{:.*}` rounds to `frac_digits`; a negative sign, if any, rides on the
        // integer part and is preserved by splitting on the ASCII '.'.
        let fixed = format!("{value:.frac_digits$}");
        let (int_part, frac_part) = match fixed.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (fixed.as_str(), None),
        };
        let (sign, digits) = match int_part.strip_prefix('-') {
            Some(rest) => ("-", rest),
            None => ("", int_part),
        };
        let grouped = group_digits(digits, conventions.group);
        match frac_part {
            Some(frac) => format!("{sign}{grouped}{}{frac}", conventions.decimal),
            None => format!("{sign}{grouped}"),
        }
    }

    /// A percentage. Spacing is locale-dependent and lives in the catalog:
    /// French writes `42 %` with a narrow no-break space English does not have.
    pub fn percent(self, whole: u64) -> String {
        self.strings().format_percent(&self.count(whole))
    }

    /// A byte size. The number follows the formatting locale; the unit WORD
    /// follows the text locale, because French writes `octets` (o/Ko/Mo/Go) and
    /// that is vocabulary, not a numeric convention (the accepted cross-client
    /// deviation, §5.3).
    pub fn bytes(self, n: u64) -> String {
        let strings = self.strings();
        const KB: u64 = 1024;
        const MB: u64 = 1024 * 1024;
        const GB: u64 = 1024 * 1024 * 1024;
        if n < KB {
            strings.format_bytes_b(&self.count(n))
        } else if n < MB {
            strings.format_bytes_kb(&self.count((n + KB / 2) / KB))
        } else if n < GB {
            strings.format_bytes_mb(&self.decimal(n as f64 / MB as f64, 1))
        } else {
            strings.format_bytes_gb(&self.decimal(n as f64 / GB as f64, 1))
        }
    }

    /// Today / Yesterday, from the text locale (vocabulary). Real calendar dates
    /// for older days need a date library and arrive with the first product
    /// surface that renders them (spec §14 Q2); the foundation ships the two
    /// relative labels the design system uses for dividers.
    pub fn today(self) -> &'static str {
        self.strings().format_today()
    }

    /// The "yesterday" divider label, from the text locale.
    pub fn yesterday(self) -> &'static str {
        self.strings().format_yesterday()
    }
}

/// Insert `separator` every three digits from the right of a run of ASCII
/// digits. Operates on the digit string only (sign and fraction handled by the
/// caller), so it is convention-agnostic and reused by `count` and `decimal`.
fn group_digits(digits: &str, separator: &str) -> String {
    let bytes = digits.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3 * separator.len());
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (len - index).is_multiple_of(3) {
            out.push_str(separator);
        }
        out.push(*byte as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_groups_thousands_per_convention() {
        assert_eq!(Formats::new(Locale::En, Locale::En).count(1234), "1,234");
        assert_eq!(
            Formats::new(Locale::En, Locale::En).count(1234567),
            "1,234,567"
        );
        assert_eq!(
            Formats::new(Locale::En, Locale::Fr).count(1234),
            "1\u{202f}234"
        );
        assert_eq!(Formats::new(Locale::En, Locale::En).count(0), "0");
        assert_eq!(Formats::new(Locale::En, Locale::En).count(999), "999");
    }

    #[test]
    fn decimal_uses_the_formatting_separators() {
        // The canonical "1 234,56" vs "1,234.56" split.
        assert_eq!(
            Formats::new(Locale::En, Locale::Fr).decimal(1234.56, 2),
            "1\u{202f}234,56"
        );
        assert_eq!(
            Formats::new(Locale::Fr, Locale::En).decimal(1234.56, 2),
            "1,234.56"
        );
    }

    #[test]
    fn percent_spacing_is_french_narrow_space() {
        // English: no space. French: U+202F before the sign.
        assert_eq!(Formats::new(Locale::En, Locale::En).percent(42), "42%");
        assert_eq!(
            Formats::new(Locale::Fr, Locale::Fr).percent(42),
            "42\u{202f}%"
        );
        // Independently switchable: French words, English formatting still keep
        // the French percent spacing (spacing is text-locale vocabulary here,
        // via the catalog) — the point of the seam is that each fact is owned by
        // exactly one locale.
        assert_eq!(
            Formats::new(Locale::Fr, Locale::En).percent(42),
            "42\u{202f}%"
        );
    }

    #[test]
    fn bytes_units_follow_the_text_locale() {
        // English units, French grouping.
        assert_eq!(Formats::new(Locale::En, Locale::En).bytes(512), "512 B");
        assert_eq!(Formats::new(Locale::En, Locale::En).bytes(2048), "2 KB");
        // French `octets` abbreviations, regardless of formatting locale.
        assert_eq!(Formats::new(Locale::Fr, Locale::En).bytes(512), "512 o");
        assert_eq!(Formats::new(Locale::Fr, Locale::En).bytes(2048), "2 Ko");
    }

    #[test]
    fn day_labels_follow_the_text_locale() {
        assert_eq!(Formats::new(Locale::En, Locale::Fr).today(), "Today");
        assert_eq!(
            Formats::new(Locale::Fr, Locale::En).today(),
            "Aujourd\u{2019}hui"
        );
    }
}
