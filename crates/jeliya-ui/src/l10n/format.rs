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
//!
//! Scope of THIS foundation slice: the full §4-D4 EN/FR convention table —
//! number, percent, and byte formatting, the `Today`/`Yesterday` day-divider
//! vocabulary, `clock` (English 12-hour / French 24-hour), `date` (day +
//! localized month name, `2 January` / `2 janvier`), and `rel_time` (relative
//! age, `just now` / `{n}m ago` / `il y a {n} min`). Only broad CLDR coverage for
//! an arbitrary THIRD formatting locale (e.g. `de-CH`, which React got free from
//! `Intl`) is still deferred — to a product-surface slice and its `icu4x`
//! dependency decision (spec §14 Q2). The seam accepts any tag upstream, so
//! widening later changes this table, not the call sites — the property the seam
//! exists to guarantee.

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
    ///
    /// Rounds half **away from zero** to match the other client's
    /// `Intl.NumberFormat` (default `halfExpand`), NOT Rust's `{:.*}` ties-to-even:
    /// `decimal(2.25, 1)` is `2.3`, not `2.2` — and so `bytes(2_359_296)` (exactly
    /// 2.25 MiB) shows `2.3 MB`, matching the React client.
    ///
    /// Rounds on the DECIMAL value, not a binary `* 10^n` scaling. `Intl` rounds
    /// the shortest decimal that round-trips to the `f64` (what the user typed), so
    /// `decimal(1.005, 2)` must be `1.01` like `Intl` — a binary scale gives
    /// `1.005 * 100 = 100.4999…` and would round to `1.00`, differing across
    /// clients. Rust's `{}` yields that shortest decimal; [`round_decimal_string`]
    /// then rounds its DIGITS, so no binary representation error creeps in.
    pub fn decimal(self, value: f64, frac_digits: usize) -> String {
        let conventions = self.conventions();
        let negative = value.is_sign_negative() && value != 0.0;
        let (int_digits, frac) = round_decimal_string(value.abs(), frac_digits);
        let grouped = group_digits(&int_digits, conventions.group);
        let sign = if negative { "-" } else { "" };
        if frac_digits == 0 {
            format!("{sign}{grouped}")
        } else {
            format!("{sign}{grouped}{}{frac}", conventions.decimal)
        }
    }

    /// A percentage. The spacing before `%` is a FORMATTING convention (like the
    /// group/decimal separators), not vocabulary: French writes `42 %` with a
    /// narrow no-break space, English writes `42%`. So it follows the FORMATTING
    /// locale, not the text locale — text=EN/format=FR yields `42 %`, and
    /// text=FR/format=EN yields `42%`.
    pub fn percent(self, value: f64, frac_digits: usize) -> String {
        // Accept a FRACTIONAL value and the caller's precision (`frac_digits`), so
        // `12.3456` with 4 digits renders `12.3456%` / `12,3456 %` — the retiring
        // client preserves fractional percentages, and a `u64`-only API could not
        // represent them. Reuses `decimal` (grouping + locale-aware rounding);
        // `format_percent` then applies the locale's percent spacing.
        catalog_for(self.formatting).format_percent(&self.decimal(value, frac_digits))
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

    /// Today / Yesterday, from the text locale (vocabulary) — the relative day
    /// dividers the design system uses. Absolute dates use [`Formats::date`].
    pub fn today(self) -> &'static str {
        self.strings().format_today()
    }

    /// The "yesterday" divider label, from the text locale.
    pub fn yesterday(self) -> &'static str {
        self.strings().format_yesterday()
    }

    /// A clock time under the FORMATTING locale's convention: English 12-hour with
    /// AM/PM (`2:30 PM`), French 24-hour (`14:30`). 12h-vs-24h is a regional
    /// FORMATTING fact (§4-D4), so it follows the formatting locale. Inputs are
    /// wrapped into range (`hour24 % 24`, `minute % 60`) so a caller cannot panic
    /// this seam.
    pub fn clock(self, hour24: u32, minute: u32) -> String {
        let hour24 = hour24 % 24;
        let minute = minute % 60;
        match self.formatting {
            Locale::En => {
                let (hour12, period) = match hour24 {
                    0 => (12, "AM"),
                    1..=11 => (hour24, "AM"),
                    12 => (12, "PM"),
                    _ => (hour24 - 12, "PM"),
                };
                format!("{hour12}:{minute:02} {period}")
            }
            Locale::Fr => format!("{hour24:02}:{minute:02}"),
        }
    }

    /// A day + month in the day-month ORDER both supported locales use (§4-D4):
    /// `2 January` / `2 janvier`. The month NAME is vocabulary (text locale); the
    /// day-month order is the shared convention. An out-of-range month has no name
    /// (`Catalog::month_name` returns `None`), so it degrades to the bare day
    /// rather than panicking — callers pass a validated 1..=12.
    pub fn date(self, day: u32, month: u32) -> String {
        match self.strings().month_name(month) {
            Some(name) => format!("{day} {name}"),
            None => day.to_string(),
        }
    }

    /// A relative age — `just now` / `{n}m ago` / `{n}h ago` / `{n}d ago`
    /// (`à l’instant` / `il y a {n} min` / `il y a {n} h` / `il y a {n} j`) — for
    /// display only, NEVER a liveness claim. The vocabulary follows the TEXT
    /// locale; the number is grouped under the FORMATTING locale (via
    /// [`Formats::count`]), the same text-vs-formatting split as the rest of the
    /// seam.
    ///
    /// Takes the already-elapsed milliseconds so the seam stays pure (the wall
    /// clock is the caller's — and the platform's — concern, never a shared
    /// component's). `elapsed_ms` is SIGNED and CLAMPED at zero: a negative value
    /// (a future timestamp from clock skew or a bad caller) renders `just now`,
    /// never `-2m ago`. Buckets and half-up rounding mirror the retiring client
    /// (`ui/src/l10n/formats.ts`): `< 45s` → just now; else minutes `< 60`; else
    /// hours `< 24`; else days.
    pub fn rel_time(self, elapsed_ms: i64) -> String {
        let strings = self.strings();
        let delta = elapsed_ms.max(0);
        // Half-up rounding on non-negative integer milliseconds (matches JS
        // `Math.round`): add half the unit before the truncating division.
        const MIN: i64 = 60_000;
        const HOUR: i64 = 3_600_000;
        const DAY: i64 = 86_400_000;
        if delta < 45_000 {
            return strings.format_just_now().to_owned();
        }
        let mins = (delta + MIN / 2) / MIN;
        if mins < 60 {
            return strings.format_minutes_ago(&self.count(mins as u64));
        }
        let hours = (delta + HOUR / 2) / HOUR;
        if hours < 24 {
            return strings.format_hours_ago(&self.count(hours as u64));
        }
        let days = (delta + DAY / 2) / DAY;
        strings.format_days_ago(&self.count(days as u64))
    }
}

/// Round the shortest decimal representation of a NON-NEGATIVE `value` to
/// `places` fractional digits, half away from zero, returning
/// `(integer_digits, fractional_digits)` as digit strings (the fraction padded to
/// exactly `places`). Rounds the DECIMAL value — the shortest string that
/// round-trips to the `f64`, which is what `Intl.NumberFormat` rounds — rather
/// than a binary `* 10^n` scale, so inputs like `1.005` (stored as `1.00499…`)
/// round to `1.01`, matching the other client instead of drifting to `1.00`.
fn round_decimal_string(value: f64, places: usize) -> (String, String) {
    // `{}` gives the shortest round-tripping decimal, never scientific notation
    // for the finite magnitudes this seam formats. Guard non-finite defensively.
    if !value.is_finite() {
        return ("0".to_owned(), "0".repeat(places));
    }
    let text = format!("{value}");
    let (int_part, frac_part) = match text.split_once('.') {
        Some((int_part, frac_part)) => (int_part.to_owned(), frac_part.to_owned()),
        None => (text, String::new()),
    };
    // Enough fractional digits already? Just pad to `places`, no rounding.
    if frac_part.len() <= places {
        let mut frac = frac_part;
        while frac.len() < places {
            frac.push('0');
        }
        return (int_part, frac);
    }
    // Keep `places` fractional digits; the first dropped digit decides rounding.
    // `halfExpand` (Intl's default) rounds up on a first-dropped digit >= 5 for a
    // non-negative magnitude, regardless of the tail.
    let round_up = frac_part.as_bytes()[places] >= b'5';
    // One digit vector spanning integer + kept fraction, for carry propagation.
    let mut digits: Vec<u8> = int_part
        .bytes()
        .chain(frac_part.bytes().take(places))
        .map(|b| b - b'0')
        .collect();
    if round_up {
        let mut at = digits.len();
        loop {
            if at == 0 {
                digits.insert(0, 1);
                break;
            }
            at -= 1;
            if digits[at] == 9 {
                digits[at] = 0;
            } else {
                digits[at] += 1;
                break;
            }
        }
    }
    let split = digits.len() - places;
    let to_str = |ds: &[u8]| ds.iter().map(|d| (d + b'0') as char).collect::<String>();
    // Strip leading zeros from the integer part, keeping at least one digit.
    let int_str = to_str(&digits[..split]);
    let int_norm = int_str.trim_start_matches('0');
    let int_norm = if int_norm.is_empty() { "0" } else { int_norm };
    (int_norm.to_owned(), to_str(&digits[split..]))
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
    fn decimal_rounds_half_away_from_zero_like_intl() {
        // Rust's `{:.*}` is ties-to-even (`2.25` → `2.2`); the other client's
        // Intl.NumberFormat is ties-AWAY (`2.25` → `2.3`). We must match Intl.
        // These are EXACT halves in f64 (…x5 that is representable), so ties-away
        // vs ties-to-even genuinely differ — the cases that matter.
        let en = Formats::new(Locale::En, Locale::En);
        assert_eq!(en.decimal(2.25, 1), "2.3");
        assert_eq!(en.decimal(0.25, 1), "0.3");
        assert_eq!(en.decimal(0.05, 1), "0.1");
        assert_eq!(en.decimal(0.5, 0), "1");
        assert_eq!(en.decimal(-2.25, 1), "-2.3");
        // Non-tie values are unaffected; padding a bare fraction still works.
        assert_eq!(en.decimal(2.24, 1), "2.2");
        assert_eq!(en.decimal(0.0, 1), "0.0");
    }

    #[test]
    fn decimal_rounds_by_decimal_value_not_binary_scaling() {
        // Inputs whose binary `f64` sits JUST BELOW a half-way boundary (e.g.
        // `1.005` is stored as `1.00499…`). `Intl.NumberFormat` rounds the decimal
        // the user typed (`halfExpand`), so these must round UP to match the other
        // client; a binary `* 10^n` scale (`1.005 * 100 = 100.4999…`) would round
        // DOWN and diverge. Values cross-checked against `Intl.NumberFormat`.
        let en = Formats::new(Locale::En, Locale::En);
        assert_eq!(en.decimal(1.005, 2), "1.01");
        assert_eq!(en.decimal(2.005, 2), "2.01");
        assert_eq!(en.decimal(1.015, 2), "1.02");
        assert_eq!(en.decimal(1.025, 2), "1.03");
        assert_eq!(en.decimal(2.675, 2), "2.68");
        assert_eq!(en.decimal(0.005, 2), "0.01");
        // Carry propagation across the decimal point and through a run of 9s.
        assert_eq!(en.decimal(9.995, 2), "10.00");
        assert_eq!(en.decimal(99.995, 2), "100.00");
        // `percent` shares the same rounding path.
        assert_eq!(en.percent(1.005, 2), "1.01%");
        // Grouping still applies after decimal rounding under fr formatting.
        assert_eq!(
            Formats::new(Locale::En, Locale::Fr).decimal(1234.005, 2),
            "1\u{202f}234,01"
        );
    }

    #[test]
    fn bytes_round_half_away_matching_the_other_client() {
        // 2_359_296 bytes is EXACTLY 2.25 MiB; the React client shows `2.3 MB`,
        // so this port must too (ties-to-even would show `2.2 MB`).
        assert_eq!(
            Formats::new(Locale::En, Locale::En).bytes(2_359_296),
            "2.3 MB"
        );
    }

    #[test]
    fn percent_spacing_follows_the_formatting_locale() {
        // English formatting: no space. French formatting: U+202F before `%`.
        assert_eq!(Formats::new(Locale::En, Locale::En).percent(42.0, 0), "42%");
        assert_eq!(
            Formats::new(Locale::Fr, Locale::Fr).percent(42.0, 0),
            "42\u{202f}%"
        );
        // Percent spacing is a FORMATTING convention (like the group/decimal
        // separators), NOT text vocabulary: it follows the formatting locale
        // regardless of the text locale. French words + English formatting →
        // `42%`; English words + French formatting → `42 %`.
        assert_eq!(Formats::new(Locale::Fr, Locale::En).percent(42.0, 0), "42%");
        assert_eq!(
            Formats::new(Locale::En, Locale::Fr).percent(42.0, 0),
            "42\u{202f}%"
        );
    }

    #[test]
    fn percent_preserves_fractional_values() {
        // A fractional percentage keeps its digits under the formatting locale's
        // separators (the retiring client shows `12.3456%`).
        assert_eq!(
            Formats::new(Locale::En, Locale::En).percent(12.3456, 4),
            "12.3456%"
        );
        assert_eq!(
            Formats::new(Locale::Fr, Locale::Fr).percent(12.3456, 4),
            "12,3456\u{202f}%"
        );
    }

    #[test]
    fn clock_follows_the_formatting_locale_convention() {
        let en = Formats::new(Locale::En, Locale::En);
        assert_eq!(en.clock(14, 30), "2:30 PM");
        assert_eq!(en.clock(0, 0), "12:00 AM"); // midnight
        assert_eq!(en.clock(12, 5), "12:05 PM"); // noon
        assert_eq!(en.clock(9, 0), "9:00 AM");
        let fr = Formats::new(Locale::Fr, Locale::Fr);
        assert_eq!(fr.clock(14, 30), "14:30");
        assert_eq!(fr.clock(0, 0), "00:00");
        assert_eq!(fr.clock(9, 5), "09:05");
        // 12h/24h is a FORMATTING convention, so it follows the formatting locale.
        assert_eq!(
            Formats::new(Locale::Fr, Locale::En).clock(14, 30),
            "2:30 PM"
        );
        assert_eq!(Formats::new(Locale::En, Locale::Fr).clock(14, 30), "14:30");
        // Out-of-range inputs wrap rather than panic.
        assert_eq!(en.clock(26, 61), en.clock(2, 1));
    }

    #[test]
    fn date_uses_the_text_locale_month_name_in_day_month_order() {
        assert_eq!(Formats::new(Locale::En, Locale::En).date(2, 1), "2 January");
        assert_eq!(Formats::new(Locale::Fr, Locale::Fr).date(2, 1), "2 janvier");
        // The month NAME follows the TEXT locale; the day-month order is shared.
        assert_eq!(Formats::new(Locale::Fr, Locale::En).date(14, 8), "14 août");
        assert_eq!(
            Formats::new(Locale::En, Locale::Fr).date(14, 8),
            "14 August"
        );
        // An out-of-range month has no name; the seam degrades to the bare day
        // instead of panicking or emitting a trailing space.
        assert_eq!(Formats::new(Locale::En, Locale::En).date(14, 0), "14");
        assert_eq!(Formats::new(Locale::En, Locale::En).date(14, 13), "14");
    }

    #[test]
    fn rel_time_buckets_and_rounds_like_the_retiring_client() {
        let en = Formats::new(Locale::En, Locale::En);
        // `< 45s` is "just now"; the 45s boundary rounds up into the minutes
        // bucket (round(45000/60000) = 1).
        assert_eq!(en.rel_time(0), "just now");
        assert_eq!(en.rel_time(44_999), "just now");
        assert_eq!(en.rel_time(45_000), "1m ago");
        // Half-up rounding on minutes (2.5 min → 3), matching JS `Math.round`.
        assert_eq!(en.rel_time(150_000), "3m ago");
        // Minutes roll into hours at 60, hours into days at 24.
        assert_eq!(en.rel_time(59 * 60_000), "59m ago");
        assert_eq!(en.rel_time(60 * 60_000), "1h ago");
        assert_eq!(en.rel_time(23 * 3_600_000), "23h ago");
        assert_eq!(en.rel_time(24 * 3_600_000), "1d ago");
        assert_eq!(en.rel_time(3 * 86_400_000), "3d ago");
    }

    #[test]
    fn rel_time_clamps_a_future_timestamp_to_just_now() {
        // A negative elapsed (future timestamp: clock skew or a bad caller) is
        // clamped to zero — "just now", never "-2m ago".
        let en = Formats::new(Locale::En, Locale::En);
        assert_eq!(en.rel_time(-120_000), "just now");
        let fr = Formats::new(Locale::Fr, Locale::Fr);
        assert_eq!(fr.rel_time(-1), "à l\u{2019}instant");
    }

    #[test]
    fn rel_time_vocabulary_follows_text_and_number_follows_formatting() {
        // Vocabulary from the TEXT locale, the grouped number from the FORMATTING
        // locale. French words + English grouping vs the reverse.
        assert_eq!(
            Formats::new(Locale::Fr, Locale::Fr).rel_time(5 * 60_000),
            "il y a 5 min"
        );
        assert_eq!(
            Formats::new(Locale::Fr, Locale::En).rel_time(5 * 60_000),
            "il y a 5 min"
        );
        assert_eq!(
            Formats::new(Locale::En, Locale::En).rel_time(2 * 3_600_000),
            "2h ago"
        );
        // A day count large enough to group proves the number follows the
        // FORMATTING locale: French grouping inserts U+202F, English a comma.
        assert_eq!(
            Formats::new(Locale::En, Locale::Fr).rel_time(1234 * 86_400_000),
            "1\u{202f}234d ago"
        );
        assert_eq!(
            Formats::new(Locale::Fr, Locale::En).rel_time(1234 * 86_400_000),
            "il y a 1,234 j"
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
