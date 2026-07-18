/** Motion preferences.
 *
 *  `prefers-reduced-motion` is a contract, not a hint (the WCAG floor in
 *  CONTRIBUTING.md), so every programmatic scroll in the app routes through
 *  here rather than reading `matchMedia` inline. Three call sites had drifted
 *  apart — the timeline honored the preference, the two inspector deep-link
 *  scrolls did not — which is exactly the drift one shared helper prevents.
 */

/** True when the user has asked the platform for reduced motion. Read at call
 *  time, not module load: the preference can change mid-session (an OS toggle,
 *  or Playwright's `reducedMotion` context option between tests). */
export function prefersReducedMotion(): boolean {
  // `matchMedia` is absent under some test environments (jsdom without the
  // shim); treating that as "reduce" keeps tests deterministic and errs toward
  // the accessible branch.
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return true;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/** The `behavior` any animated scroll should use: instant under reduced
 *  motion, smooth otherwise. */
export function scrollBehavior(): ScrollBehavior {
  return prefersReducedMotion() ? 'auto' : 'smooth';
}

/** Scroll an element into view honoring the motion preference. `block`
 *  defaults to `'nearest'` — the least disruptive option, which is what a
 *  deep-link reveal wants. */
export function scrollIntoView(
  el: Element,
  { block = 'nearest', inline = 'nearest' }: { block?: ScrollLogicalPosition; inline?: ScrollLogicalPosition } = {},
): void {
  el.scrollIntoView({ block, inline, behavior: scrollBehavior() });
}
