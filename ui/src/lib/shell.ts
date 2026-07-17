/** The three shells (docs/room-workbench.md, decision 3).
 *
 *  CSS owns the layout — which panes show, and the grid that places them.
 *  This module exists for the one fork CSS cannot make: the compact room app
 *  bar and the desktop room header are different elements, not one element
 *  restyled, and rendering both to hide one would put two room titles in the
 *  accessibility tree.
 *
 *  The breakpoints are therefore written twice — here and in `styles.css` —
 *  because a media query cannot read a `var()`. `shell.test.ts` parses the
 *  stylesheet and fails if the two ever disagree, which is the compiler help
 *  this duplication otherwise would not get.
 */

import { useEffect, useState } from 'react';

/** Below this, one pane at a time. 899.98 rather than 899 so that a
 *  fractional width (browser zoom, a scaled display) cannot fall into a gap
 *  between the compact and medium queries and match neither. */
export const COMPACT_MAX = 899.98;

/** At and above this, the inspector is a third column rather than a drawer. */
export const WIDE_MIN = 1280;

export type Shell = 'compact' | 'medium' | 'wide';

export const COMPACT_QUERY = `(max-width: ${COMPACT_MAX}px)`;
export const WIDE_QUERY = `(min-width: ${WIDE_MIN}px)`;

export function shellFor(width: number): Shell {
  if (width <= COMPACT_MAX) return 'compact';
  return width >= WIDE_MIN ? 'wide' : 'medium';
}

/** Track the current shell. Subscribes to `matchMedia` rather than `resize`:
 *  it fires only on the transitions that matter, not on every pixel. */
export function useShell(): Shell {
  const [shell, setShell] = useState<Shell>(() => shellFor(window.innerWidth));

  useEffect(() => {
    const compact = window.matchMedia(COMPACT_QUERY);
    const wide = window.matchMedia(WIDE_QUERY);
    const sync = () => setShell(compact.matches ? 'compact' : wide.matches ? 'wide' : 'medium');
    sync();
    compact.addEventListener('change', sync);
    wide.addEventListener('change', sync);
    return () => {
      compact.removeEventListener('change', sync);
      wide.removeEventListener('change', sync);
    };
  }, []);

  return shell;
}
