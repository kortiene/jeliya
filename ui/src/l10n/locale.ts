/** Locale selection and persistence (issue #74).
 *
 *  TWO preferences, not one (`docs/glossary-fr.md`, decision 4): the TEXT
 *  locale chooses the interface language, the FORMATTING locale chooses date,
 *  number and calendar conventions. Either may be unset, and unset means
 *  "follow the platform" — a documented fallback rather than a silent default
 *  to English.
 *
 *  Storage follows the pattern already used by `lib/names.ts`,
 *  `lib/roomFlags.ts` and `lib/lastSeen.ts`: a `jeliya.*` key, read and written
 *  through try/catch because storage can be unavailable (private browsing, a
 *  disabled cookie policy) and a preference is never worth an exception.
 */

import { resolveFormattingLocale } from './formats';

const TEXT_KEY = 'jeliya.textLocale';
const FORMATTING_KEY = 'jeliya.formattingLocale';

/** The locales with a complete catalog. A tag outside this set falls back;
 *  shipping a half-translated interface is worse than shipping one language. */
export const SUPPORTED_LOCALES = ['en', 'fr'] as const;
export type SupportedLocale = (typeof SUPPORTED_LOCALES)[number];

export const FALLBACK_LOCALE: SupportedLocale = 'en';

export function isSupported(tag: string | null | undefined): tag is SupportedLocale {
  return tag != null && (SUPPORTED_LOCALES as readonly string[]).includes(tag);
}

function platformLanguageTags(): readonly string[] {
  if (typeof navigator === 'undefined') return [];
  return navigator.languages?.length ? navigator.languages : [navigator.language];
}

/** The platform's preferred language, narrowed to one we have a catalog for.
 *
 *  Matches on the PRIMARY subtag, so `fr-CA` and `fr-BE` both resolve to the
 *  French catalog — a Canadian French speaker should not silently get English
 *  because the region differs. */
export function platformLocale(): SupportedLocale {
  for (const tag of platformLanguageTags()) {
    if (!tag) continue;
    const primary = tag.split('-')[0].toLowerCase();
    if (isSupported(primary)) return primary;
  }
  return FALLBACK_LOCALE;
}

/** The platform locale used for dates and numbers. Unlike `platformLocale`,
 *  this is not narrowed to a language with a text catalog: `de-CH`, for
 *  example, is a perfectly valid formatting choice while the UI reads in
 *  English or French. Invalid/unsupported browser tags are skipped. */
export function platformFormattingLocale(fallback: string = FALLBACK_LOCALE): string {
  for (const tag of platformLanguageTags()) {
    if (!tag) continue;
    try {
      if (Intl.DateTimeFormat.supportedLocalesOf([tag]).length > 0) return tag;
    } catch {
      /* try the next platform tag */
    }
  }
  return fallback;
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    /* a preference is never worth throwing over */
  }
}

/** The stored TEXT-locale preference, or null for "follow the platform". */
export function loadTextLocale(): SupportedLocale | null {
  const stored = read(TEXT_KEY);
  return isSupported(stored) ? stored : null;
}

export function saveTextLocale(tag: SupportedLocale | null): void {
  write(TEXT_KEY, tag);
}

/** The stored FORMATTING-locale preference, or null for "follow the system".
 *  Any BCP 47 tag is allowed here — unlike the text locale it needs no
 *  catalog, only `Intl` support, so a user may read English while formatting
 *  under `de-CH`. */
export function loadFormattingLocale(): string | null {
  return read(FORMATTING_KEY);
}

export function saveFormattingLocale(tag: string | null): void {
  write(FORMATTING_KEY, tag);
}

export interface ResolvedLocales {
  /** The catalog to render copy from. */
  text: SupportedLocale;
  /** The tag `Intl` formats under. */
  formatting: string;
  /** True when the text locale came from the platform rather than a choice —
   *  the Settings picker shows this as "follow system" rather than pretending
   *  the user picked it. */
  textFollowsSystem: boolean;
}

/** Resolve both locales from storage plus the platform. */
export function resolveLocales(): ResolvedLocales {
  const storedText = loadTextLocale();
  const text = storedText ?? platformLocale();
  return {
    text,
    formatting: resolveFormattingLocale(loadFormattingLocale(), text, platformFormattingLocale(text)),
    textFollowsSystem: storedText === null,
  };
}
