/** Which pane is *the page* — the single `main` landmark, per shell.
 *
 *  The shell keeps every pane mounted and hides the inactive ones with CSS
 *  (DESIGN.md: "Panes hide, they do not unmount" — timeline scroll and
 *  selection must survive a shell change, an inspector toggle, and a
 *  destination switch). That is the right layout behaviour and the wrong
 *  landmark behaviour: `<main>` lived permanently on the workspace, so the four
 *  destinations that hide the workspace — compact Rooms, compact inspector,
 *  Fleet, Settings — exposed no `main` at all, while Fleet and Settings
 *  offered a `region` in its place (issue #72).
 *
 *  Rather than move markup around (which would remount the panes and lose the
 *  very state the hide-don't-unmount rule protects), the landmark ROLE moves:
 *  exactly one mounted-and-visible pane carries `main` for the current route
 *  and shell, and the rest carry their structural role or none. Roles are
 *  attributes, so switching one never remounts a subtree.
 *
 *  The rule is data, not markup, so it can be unit-tested exhaustively across
 *  every pane x shell pair — see `landmarks.test.ts`.
 */

import type { Shell } from './shell';

/** The panes the shell can show. Mirrors the `pane` union in `App.tsx`, which
 *  is itself derived from the route (docs/room-workbench.md, decision 2). */
export type Pane = 'rooms' | 'room' | 'inspector' | 'fleet' | 'settings';

/** The elements that can carry the page's `main` landmark. */
export type PageRegion = 'sidebar' | 'center' | 'inspector' | 'fleet' | 'settings';

export interface DocumentTitleLabels {
  /** Global destination labels come from the current text catalog. */
  rooms: string;
  fleet: string;
  settings: string;
  /** The brand is normally the non-translatable BRAND token. */
  app: string;
}

/** The one pane that is the page — the `main` landmark and the target of the
 *  "skip to workspace" link.
 *
 *  - Fleet and Settings are always their own page when routed to.
 *  - Compact shows one pane at a time, so whichever pane is displayed is the
 *    page: the room rail on `/rooms`, the inspector on a room tool.
 *  - Medium and wide both keep the workspace live beside the inspector, so the
 *    workspace stays the page and the inspector stays `complementary`. Medium
 *    draws the inspector as a drawer pinned to the workspace's right edge, but
 *    the workspace RESERVES that width while it is open (`.inspector-open` in
 *    styles.css), so no workspace content ever sits underneath it — which is
 *    what keeps a focused control from being hidden behind the drawer without
 *    making the workspace inert. Only compact hides the workspace outright, and
 *    there the inspector is the whole screen and therefore the page.
 */
export function pageRegion(pane: Pane, shell: Shell): PageRegion {
  if (pane === 'fleet') return 'fleet';
  if (pane === 'settings') return 'settings';
  if (pane === 'inspector' && shell === 'compact') return 'inspector';
  if (pane === 'rooms' && shell === 'compact') return 'sidebar';
  return 'center';
}

/** The document title for a destination. The SPA never updated `document.title`
 *  — every one of the six destinations announced the same static "Jeliya" — so
 *  a screen-reader user had no signal that a navigation had happened at all.
 *
 *  `roomName` is the room's display name when the route names a room. It is
 *  user data and is never translated or reformatted; an untitled room falls
 *  back to the destination alone rather than inventing a name. */
export function documentTitle(
  pane: Pane,
  {
    roomName,
    destLabel,
    labels,
  }: {
    roomName?: string | null;
    destLabel?: string | null;
    labels: DocumentTitleLabels;
  },
): string {
  if (pane === 'fleet') return `${labels.fleet} · ${labels.app}`;
  if (pane === 'settings') return `${labels.settings} · ${labels.app}`;
  if (pane === 'rooms') return `${labels.rooms} · ${labels.app}`;
  // Inside a room: name the room, and the tool when the inspector is open.
  const room = roomName?.trim() || null;
  const parts = [destLabel || null, room, labels.app].filter((p): p is string => Boolean(p));
  return parts.join(' · ');
}
