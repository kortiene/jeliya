import { afterEach, describe, expect, it, vi } from 'vitest';
import { prefersReducedMotion, scrollBehavior, scrollIntoView } from './motion';

/** Install a `matchMedia` stub that answers the reduced-motion query. */
function withMotionPreference(reduce: boolean) {
  const matchMedia = vi.fn((query: string) => ({
    matches: query.includes('prefers-reduced-motion: reduce') ? reduce : false,
    media: query,
  })) as unknown as typeof window.matchMedia;
  vi.stubGlobal('window', { ...globalThis.window, matchMedia });
  return matchMedia;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('prefersReducedMotion', () => {
  it('reports the platform preference', () => {
    withMotionPreference(true);
    expect(prefersReducedMotion()).toBe(true);
    withMotionPreference(false);
    expect(prefersReducedMotion()).toBe(false);
  });

  it('reads the preference at CALL time, not module load', () => {
    // The preference can change mid-session (an OS toggle, or Playwright
    // swapping the context option between tests). Caching it at import would
    // pin whichever value happened to be set first.
    withMotionPreference(false);
    expect(prefersReducedMotion()).toBe(false);
    withMotionPreference(true);
    expect(prefersReducedMotion()).toBe(true);
  });

  it('errs toward reduced motion when matchMedia is unavailable', () => {
    vi.stubGlobal('window', { ...globalThis.window, matchMedia: undefined });
    expect(prefersReducedMotion()).toBe(true);
  });
});

describe('scrollBehavior', () => {
  it('is instant under reduced motion and smooth otherwise', () => {
    withMotionPreference(true);
    expect(scrollBehavior()).toBe('auto');
    withMotionPreference(false);
    expect(scrollBehavior()).toBe('smooth');
  });
});

describe('scrollIntoView', () => {
  it('passes the motion-appropriate behavior through', () => {
    const el = { scrollIntoView: vi.fn() } as unknown as Element;

    withMotionPreference(true);
    scrollIntoView(el);
    expect(el.scrollIntoView).toHaveBeenLastCalledWith({ block: 'nearest', inline: 'nearest', behavior: 'auto' });

    withMotionPreference(false);
    scrollIntoView(el, { block: 'center' });
    expect(el.scrollIntoView).toHaveBeenLastCalledWith({ block: 'center', inline: 'nearest', behavior: 'smooth' });
  });
});
