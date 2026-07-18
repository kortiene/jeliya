import { describe, expect, it } from 'vitest';
import { en } from './en';
import { fr } from './fr';
import { Formats, resolveFormattingLocale } from './formats';
import { fillTemplate, templateParts } from './template';
import { isSupported, SUPPORTED_LOCALES } from './locale';
import { LANGUAGE_NAMES } from './tokens';

/** The catalog contract, asserted rather than assumed (issue #74).
 *
 *  `scripts/check-ui-i18n.mjs` is the CI gate over catalog CONTENT — parity,
 *  emptiness, French left in English, typography. These are the behaviours the
 *  gate cannot see: that formatting actually varies by locale, that the slot
 *  mechanism fails visibly rather than silently, and that the vocabulary the
 *  two clients share stays shared.
 */

const catalogs = { en, fr };

describe('catalog parity', () => {
  it('declares the same keys in every locale', () => {
    const enKeys = Object.keys(en).sort();
    for (const [tag, catalog] of Object.entries(catalogs)) {
      expect(Object.keys(catalog).sort(), `${tag} has a different key set from en`).toEqual(enKeys);
    }
  });

  it('tags each catalog with its own locale', () => {
    for (const [tag, catalog] of Object.entries(catalogs)) {
      expect(catalog.localeTag).toBe(tag);
    }
  });

  it('covers every supported locale', () => {
    for (const tag of SUPPORTED_LOCALES) {
      expect(isSupported(tag)).toBe(true);
      expect(catalogs[tag], `no catalog for the supported locale ${tag}`).toBeDefined();
    }
  });

  it('gives every supported locale an endonym for the Settings picker', () => {
    for (const tag of SUPPORTED_LOCALES) {
      expect(LANGUAGE_NAMES[tag]).toBeTruthy();
      expect(LANGUAGE_NAMES[tag]).not.toBe(tag);
    }
  });
});

describe('the French typography contract', () => {
  // docs/glossary-fr.md decision 7. These are invisible in review, which is
  // exactly why they are asserted.
  const values = Object.entries(fr).filter(([, v]) => typeof v === 'string') as [string, string][];

  it('never puts a plain space before a high punctuation mark', () => {
    for (const [key, value] of values) {
      expect(value, `fr.${key} uses a breaking space before ; ! or ?`).not.toMatch(/ [;!?]/);
      expect(value, `fr.${key} uses a breaking space before :`).not.toMatch(/ :/);
    }
  });

  it('uses the typographic apostrophe, never the ASCII one', () => {
    for (const [key, value] of values) {
      expect(value, `fr.${key} uses a straight apostrophe`).not.toMatch(/\w'\w/);
    }
  });
});

describe('the vocabulary both clients share', () => {
  it('keeps the retired status words retired', () => {
    // docs/room-workbench.md decision 4 retired these as STATE labels. The
    // lifecycle filter chip legitimately reads "Active", so this checks the
    // state keys specifically rather than banning the word outright.
    const stateKeys = Object.keys(en).filter((k) => k.startsWith('roomsState'));
    expect(stateKeys.length).toBeGreaterThan(0);
    for (const key of stateKeys) {
      expect((en as Record<string, unknown>)[key]).not.toBe('Active');
    }
  });

  it('never translates a wire path badge', () => {
    // Tier 2: the badge shows the daemon's own word, in every language, or it
    // is claiming a network fact the daemon did not report.
    expect(fr.wirePathDirect).toBe(en.wirePathDirect);
    expect(fr.wirePathRelay).toBe(en.wirePathRelay);
  });
});

describe('Formats', () => {
  it('uses the system locale when formatting follows system', () => {
    expect(resolveFormattingLocale(null, 'fr', 'en-US')).toBe('en-US');
  });

  it('keeps an explicit formatting locale ahead of the system locale', () => {
    expect(resolveFormattingLocale('de-DE', 'fr', 'en-US')).toBe('de-DE');
  });

  it('falls through an invalid stored locale without throwing', () => {
    expect(resolveFormattingLocale('not_a_locale', 'fr', 'en-US')).toBe('en-US');
  });

  it('formats byte units with the text locale s words', () => {
    // The accepted deviation carried over from Flutter: unit WORDS follow the
    // text locale (they are vocabulary), numeric conventions follow the
    // formatting locale.
    expect(new Formats(en, 'en').bytes(2048)).toMatch(/KB/);
    expect(new Formats(fr, 'fr').bytes(2048)).toMatch(/Ko/);
  });

  it('separates the text locale from the formatting locale', () => {
    // The whole point of two preferences: French words, German numbers.
    const mixed = new Formats(fr, 'de-DE');
    expect(mixed.bytes(1024 * 1024 * 3.5)).toContain('Mo'); // vocabulary: French
    expect(mixed.bytes(1024 * 1024 * 3.5)).toContain(','); // convention: German decimal comma
  });

  it('preserves signed fractional progress with localized percent spacing', () => {
    expect(new Formats(en, 'en').percent(42)).toBe('42%');
    expect(new Formats(fr, 'fr').percent(42)).toBe('42\u202f%');
    expect(new Formats(en, 'en').percent(12.3456)).toBe('12.3456%');
  });

  it('never renders a negative age from a future timestamp', () => {
    // Clock skew must read "just now", never "-2m ago".
    const f = new Formats(en, 'en');
    expect(f.relTime(Date.now() + 60_000)).toBe(en.formatJustNow);
  });

  it('keeps relative-time words in the text locale when number formatting differs', () => {
    const mixed = new Formats(fr, 'en-US');
    expect(mixed.relTime(Date.now() - 5 * 60_000)).toMatch(/^il y a 5 min$/);
  });

  it('refuses to invent a size for a non-size', () => {
    const f = new Formats(en, 'en');
    expect(f.bytes(-1)).toBe('?');
    expect(f.bytes(Number.NaN)).toBe('?');
  });
});

describe('Template', () => {
  it('splits a sentence into literals and slots', () => {
    expect(templateParts('Leaving {room} {id} publishes a departure.')).toEqual([
      { text: 'Leaving ' },
      { slot: 'room' },
      { text: ' ' },
      { slot: 'id' },
      { text: ' publishes a departure.' },
    ]);
  });

  it('renders an unknown slot literally rather than dropping it', () => {
    // A silently dropped slot is a sentence that reads fine and says the wrong
    // thing. A visible {marker} is a bug report from the screen.
    expect(fillTemplate('a {missing} b', {})).toBe('a {missing} b');
  });

  it('lets a translator reorder slots', () => {
    // The property that makes this mechanism worth having.
    const slots = { one: 'X', two: 'Y' };
    expect(fillTemplate('{one} then {two}', slots)).toBe('X then Y');
    expect(fillTemplate('{two} avant {one}', slots)).toBe('Y avant X');
  });
});
