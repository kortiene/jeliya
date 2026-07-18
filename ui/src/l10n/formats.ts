/** The single formatting seam (issue #74; `docs/i18n.md` rule 4).
 *
 *  Nothing else in the React client may format a date, a number, a byte size or
 *  a relative time. Before this, `lib/format.ts` hardcoded English words AND
 *  called `toLocaleTimeString([], …)` with an empty locale list — meaning it
 *  formatted under the BROWSER's locale while spelling its words in English, so
 *  a French user on an English browser got neither.
 *
 *  Text locale versus formatting locale
 *  ------------------------------------
 *  These are SEPARATE (`docs/glossary-fr.md`, decision 4, settled before the
 *  first string was written). Someone may read the interface in French while
 *  expecting dates and numbers in their region's conventions, or read English
 *  while living somewhere that writes 1 234,56. Collapsing them into one
 *  "locale" is the mistake this design exists to avoid, and Flutter already
 *  models it this way — `PrefsStore.textLocale` and `.formattingLocale` are two
 *  persisted preferences.
 *
 *  The accepted deviation, carried over from Flutter so the two clients agree:
 *  byte-unit WORDS and the Today/Yesterday/"ago" phrases follow the TEXT
 *  locale, because they are vocabulary. Only numeric and calendar conventions
 *  follow the formatting locale.
 *
 *  No dependency: `Intl` has been in every browser this app supports for years.
 */

import type { Catalog } from './catalog';

export class Formats {
  /** @param strings the TEXT-locale catalog — supplies unit words and phrases.
   *  @param localeTag the FORMATTING locale — supplies numeric and calendar
   *         conventions. Deliberately not `strings.localeTag`. */
  constructor(
    private readonly strings: Catalog,
    readonly localeTag: string,
  ) {}

  /** Clock time, e.g. "9:41 AM" or "09:41". */
  clock(ts: number): string {
    return new Intl.DateTimeFormat(this.localeTag, { hour: 'numeric', minute: '2-digit' }).format(ts);
  }

  /** A day divider: Today / Yesterday from the TEXT locale, otherwise a date
   *  under the FORMATTING locale's calendar conventions. */
  dayLabel(ts: number): string {
    const date = new Date(ts);
    const today = new Date();
    const yesterday = new Date(today);
    yesterday.setDate(today.getDate() - 1);
    if (date.toDateString() === today.toDateString()) return this.strings.formatToday;
    if (date.toDateString() === yesterday.toDateString()) return this.strings.formatYesterday;
    return new Intl.DateTimeFormat(this.localeTag, { month: 'short', day: 'numeric', year: 'numeric' }).format(date);
  }

  /** Byte sizes. Unit words come from the catalog because French writes octets
   *  (o/Ko/Mo/Go), not bytes — the number's decimal separator comes from the
   *  formatting locale. `?` for a value that is not a real size, never a
   *  fabricated 0. */
  bytes(n: number): string {
    if (!Number.isFinite(n) || n < 0) return '?';
    const num = (value: number, digits = 0) =>
      new Intl.NumberFormat(this.localeTag, {
        minimumFractionDigits: digits,
        maximumFractionDigits: digits,
      }).format(value);
    if (n < 1024) return this.strings.formatBytesB(num(n));
    if (n < 1024 * 1024) return this.strings.formatBytesKb(num(Math.round(n / 1024)));
    if (n < 1024 ** 3) return this.strings.formatBytesMb(num(n / 1024 ** 2, 1));
    return this.strings.formatBytesGb(num(n / 1024 ** 3, 1));
  }

  /** A whole number under the formatting locale's grouping conventions. */
  count(n: number): string {
    return new Intl.NumberFormat(this.localeTag).format(n);
  }

  /** A percentage. Renders through the catalog because SPACING is
   *  locale-dependent: French writes "42 %" with a narrow no-break space that
   *  English does not have. */
  percent(n: number): string {
    const formatted = new Intl.NumberFormat(this.localeTag, {
      minimumFractionDigits: Number.isInteger(n) ? 0 : 1,
      maximumFractionDigits: Number.isInteger(n) ? 0 : 4,
    }).format(n);
    return this.strings.formatPercent(formatted);
  }

  /** Relative time from a real event timestamp — display only, never a
   *  liveness claim.
   *
   *  Clamped at zero: a timestamp in the future (clock skew, a bad caller)
   *  must render "just now", never "-2m ago". */
  relTime(ts: number): string {
    const delta = Math.max(0, Date.now() - ts);
    if (delta < 45_000) return this.strings.formatJustNow;
    const mins = Math.round(delta / 60_000);
    if (mins < 60) return this.strings.formatMinutesAgo(this.count(mins));
    const hours = Math.round(delta / 3_600_000);
    if (hours < 24) return this.strings.formatHoursAgo(this.count(hours));
    return this.strings.formatDaysAgo(this.count(Math.round(delta / 86_400_000)));
  }
}

/** Resolve the formatting locale actually in effect.
 *
 *  Order: the explicit preference, else the platform formatting locale, else
 *  the text locale. The platform argument is optional so pure callers and
 *  tests can make the fallback deterministic.
 *  A tag the runtime cannot honor falls back rather than throwing — a bad
 *  stored preference must not brick the interface. */
export function resolveFormattingLocale(
  preferred: string | null,
  textLocale: string,
  platformLocale?: string | null,
): string {
  const candidates = [preferred, platformLocale, textLocale].filter((t): t is string => Boolean(t));
  for (const tag of candidates) {
    try {
      // Throws RangeError on a malformed tag; returns [] when unsupported.
      if (Intl.DateTimeFormat.supportedLocalesOf([tag]).length > 0) return tag;
    } catch {
      /* try the next candidate */
    }
  }
  return textLocale;
}
