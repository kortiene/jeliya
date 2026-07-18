/** Render-time copy resolution (issue #74; `docs/i18n.md` rule 1).
 *
 *  Copy is resolved AT RENDER TIME through `useStrings()`, never captured into
 *  component state. That is what makes a language switch apply live: every
 *  consumer re-reads on the next render, so there is no restart and no stale
 *  half-translated screen. Flutter's `context.strings` is the same contract.
 *
 *  `useFormats()` is its companion for anything numeric or temporal, under the
 *  separately-chosen formatting locale.
 */

import { createContext, useContext, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import type { Catalog } from './catalog';
import { Formats, resolveFormattingLocale } from './formats';
import {
  FALLBACK_LOCALE,
  loadFormattingLocale,
  loadTextLocale,
  platformFormattingLocale,
  resolveLocales,
  saveFormattingLocale,
  saveTextLocale,
} from './locale';
import type { SupportedLocale } from './locale';
import { en } from './en';
import { fr } from './fr';

const CATALOGS: Record<SupportedLocale, Catalog> = { en, fr };

export interface L10n {
  /** The catalog for the current TEXT locale. */
  strings: Catalog;
  /** Formatting under the current FORMATTING locale. */
  formats: Formats;
  /** The chosen text locale, or null when following the platform. */
  textLocale: SupportedLocale | null;
  /** The chosen formatting locale, or null when following the platform. */
  formattingLocale: string | null;
  /** The tags actually in effect, after fallback. */
  resolved: { text: SupportedLocale; formatting: string };
  setTextLocale(tag: SupportedLocale | null): void;
  setFormattingLocale(tag: string | null): void;
}

const L10nContext = createContext<L10n | null>(null);

export function L10nProvider({ children }: { children: ReactNode }) {
  const [textLocale, setTextState] = useState<SupportedLocale | null>(loadTextLocale);
  const [formattingLocale, setFormattingState] = useState<string | null>(loadFormattingLocale);
  const [platformRevision, setPlatformRevision] = useState(0);

  const value = useMemo<L10n>(() => {
    const platform = resolveLocales();
    const text = textLocale ?? platform.text;
    const formatting = resolveFormattingLocale(formattingLocale, text, platformFormattingLocale(text));
    const strings = CATALOGS[text] ?? CATALOGS[FALLBACK_LOCALE];
    return {
      strings,
      formats: new Formats(strings, formatting),
      textLocale,
      formattingLocale,
      resolved: { text, formatting },
      setTextLocale: (tag) => {
        saveTextLocale(tag);
        setTextState(tag);
      },
      setFormattingLocale: (tag) => {
        saveFormattingLocale(tag);
        setFormattingState(tag);
      },
    };
  }, [textLocale, formattingLocale, platformRevision]);

  // A browser can change language while the app is open. System-following
  // preferences should react just like an explicit picker change, without a
  // restart; explicit preferences are re-resolved to the same values.
  useEffect(() => {
    const onLanguageChange = () => setPlatformRevision((revision) => revision + 1);
    window.addEventListener('languagechange', onLanguageChange);
    return () => window.removeEventListener('languagechange', onLanguageChange);
  }, []);

  // Keep the document in step with the interface. `lang` drives the screen
  // reader's pronunciation and the browser's own hyphenation — a French
  // interface announced by an English voice is the failure this prevents.
  useEffect(() => {
    document.documentElement.lang = value.resolved.text;
  }, [value.resolved.text]);

  return <L10nContext.Provider value={value}>{children}</L10nContext.Provider>;
}

function useL10n(): L10n {
  const ctx = useContext(L10nContext);
  if (ctx === null) {
    // A missing provider is a wiring bug, and silently falling back to English
    // would hide it until someone switched language in production.
    throw new Error('useStrings/useFormats used outside <L10nProvider>');
  }
  return ctx;
}

/** The catalog for the current text locale. Call it in render, never store it. */
export function useStrings(): Catalog {
  return useL10n().strings;
}

/** Formatting under the current formatting locale. */
export function useFormats(): Formats {
  return useL10n().formats;
}

/** Locale selection, for the Settings language card. */
export function useLocaleSettings(): L10n {
  return useL10n();
}

/** Test/root helper: build an L10n value without React state. */
export function makeL10n(text: SupportedLocale, formatting?: string): Omit<L10n, 'setTextLocale' | 'setFormattingLocale'> {
  const strings = CATALOGS[text];
  const tag = resolveFormattingLocale(formatting ?? null, text);
  return {
    strings,
    formats: new Formats(strings, tag),
    textLocale: text,
    formattingLocale: formatting ?? null,
    resolved: { text, formatting: tag },
  };
}

export { CATALOGS };
