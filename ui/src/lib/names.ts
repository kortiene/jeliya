// Local-only sender naming. Aliases live in localStorage keyed by
// identity_id and never leave this machine — the protocol has no display
// names, so we never pretend otherwise.

const STORAGE_KEY = 'jeliya.aliases.v1';

export function loadAliases(): Record<string, string> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== 'object' || parsed === null) return {};
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof v === 'string') out[k] = v;
    }
    return out;
  } catch {
    return {};
  }
}

export function saveAliases(aliases: Record<string, string>): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(aliases));
  } catch {
    // storage full/blocked — aliases just won't persist
  }
}

/** Non-persistent fallback names. Seeded by the mock fixture only, so the
 *  demo content echoes the mockups; empty against a real daemon. A user
 *  alias always wins over a suggestion. */
export const suggestedNames: Record<string, string> = {};
