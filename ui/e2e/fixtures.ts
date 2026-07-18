import { expect, test as base } from '@playwright/test';
import type { Page } from '@playwright/test';
import { en } from '../src/l10n/en';

/** COPY versus DATA — the distinction the whole file turns on (`docs/i18n.md`,
 *  rule 6).
 *
 *  COPY is what the interface says: destination labels, filter chips, button
 *  names. It is translated, so a spec must never contain it as a literal —
 *  every occurrence resolves through the shared `en` catalog, and the same
 *  spec then runs unchanged under a French text locale by swapping the catalog
 *  the helpers read.
 *
 *  DATA is what the fixture contains: the room names in `MOCK_ROOMS`, message
 *  bodies, file names. It is NOT translated — `mock.ts` serves the same bytes
 *  in every locale — so pinning it as a literal is exactly right, and routing
 *  it through a catalog would be a category error (a translator would be handed
 *  "Build Iroh Rooms MVP" to localize).
 *
 *  The rule of thumb: if a French user would read something different there, it
 *  is copy and belongs in the catalog. */

/** The shell boundaries (docs/room-workbench.md, decision 3; ui/src/lib/shell.ts).
 *  Compact stops just below 900 — 900 itself is the first medium width, which
 *  is why issue #62 names 899 and 900 as separate cases. */
export const COMPACT_MAX_WIDTH = 899;
export const WIDE_MIN_WIDTH = 1280;

export type Shell = 'compact' | 'medium' | 'wide';

export function shellForWidth(width: number): Shell {
  if (width <= COMPACT_MAX_WIDTH) return 'compact';
  return width >= WIDE_MIN_WIDTH ? 'wide' : 'medium';
}

/* -- destinations, as KEYS rather than English labels ------------------------
 *
 *  These used to be string-literal unions of the English labels:
 *
 *      export type GlobalDest = 'Rooms' | 'Agent Fleet' | 'Settings';
 *
 *  which made every spec depend on English at the TYPE level, not merely at
 *  runtime: no spec could be parameterized by locale while `'Agent Fleet'` was
 *  the only value the compiler would accept. A destination is now named by its
 *  CATALOG KEY, and the label is resolved from `en` at use — so pointing the
 *  helpers at `fr` is a one-line change here rather than an edit in 20 specs.
 *
 *  MIGRATION PATH. The ~20 existing specs pass the label (`navigate('Agent
 *  Fleet')`, `roomTab('Files')`), and they are not edited in this change, so
 *  the English label stays accepted as a DEPRECATED ALIAS that maps to the key.
 *  Both forms resolve through the same catalog lookup — the alias only decides
 *  which key you meant, it never becomes the selector text. To finish:
 *
 *   1. Per spec, replace each label argument with its key
 *      (`'Agent Fleet'` → `'destFleet'`, `'Files'` → `'roomDestFiles'`).
 *   2. Delete the three `*_ALIAS` maps and the alias arm of each `Dest` union;
 *      the compiler then lists every remaining label call site for you.
 *   3. Add the locale dimension to the Playwright projects and have `destKey`
 *      read the catalog for the project's locale instead of `en`.
 *
 *  Step 3 is the point of the exercise; steps 1 and 2 are mechanical. */

/** The global destinations — the only three (docs/room-workbench.md). */
export type GlobalDestKey = 'destRooms' | 'destFleet' | 'destSettings';

/** The room destinations. */
export type RoomDestKey =
  | 'roomDestActivity'
  | 'roomDestPeople'
  | 'roomDestAgents'
  | 'roomDestFiles'
  | 'roomDestPipes';

/** The rooms-list lifecycle filter chips. */
export type RoomFilterKey = 'roomsFilterAll' | 'roomsFilterActive' | 'roomsFilterDeparted';

/** @deprecated Pass the key instead. Kept so the unmigrated specs compile. */
const GLOBAL_DEST_ALIAS = {
  Rooms: 'destRooms',
  'Agent Fleet': 'destFleet',
  Settings: 'destSettings',
} as const satisfies Record<string, GlobalDestKey>;

/** @deprecated Pass the key instead. Kept so the unmigrated specs compile. */
const ROOM_DEST_ALIAS = {
  Activity: 'roomDestActivity',
  People: 'roomDestPeople',
  'Agents & Runs': 'roomDestAgents',
  Files: 'roomDestFiles',
  Pipes: 'roomDestPipes',
} as const satisfies Record<string, RoomDestKey>;

/** @deprecated Pass the key instead. Kept so the unmigrated specs compile. */
const ROOM_FILTER_ALIAS = {
  All: 'roomsFilterAll',
  Active: 'roomsFilterActive',
  'Left & removed': 'roomsFilterDeparted',
} as const satisfies Record<string, RoomFilterKey>;

/** A destination, named by catalog key or — deprecated — by English label. */
export type GlobalDest = GlobalDestKey | keyof typeof GLOBAL_DEST_ALIAS;
export type RoomDest = RoomDestKey | keyof typeof ROOM_DEST_ALIAS;
export type RoomFilter = RoomFilterKey | keyof typeof ROOM_FILTER_ALIAS;

/** @deprecated The old name for {@link RoomDest}. */
export type RoomDestLabel = RoomDest;

/** Normalize either spelling to the catalog key. A key passes through; a label
 *  is looked up. The two sets are disjoint by construction (keys are
 *  lowerCamelCase, labels are the displayed words), so this can never be
 *  ambiguous. */
function destKey<K extends string>(alias: Record<string, K>, dest: string): K {
  return (alias as Record<string, K | undefined>)[dest] ?? (dest as K);
}

/** The displayed label for a global destination, from the shared catalog. */
export function globalDestLabel(dest: GlobalDest): string {
  return en[destKey(GLOBAL_DEST_ALIAS, dest)];
}

/** The displayed label for a room destination, from the shared catalog. */
export function roomDestLabel(dest: RoomDest): string {
  return en[destKey(ROOM_DEST_ALIAS, dest)];
}

/** The displayed label for a lifecycle filter chip, from the shared catalog. */
export function roomFilterLabel(filter: RoomFilter): string {
  return en[destKey(ROOM_FILTER_ALIAS, filter)];
}

/** Room names from the populated mock fixture (ui/src/lib/mock.ts). Each is
 *  UNIQUE, so `roomItem(name)` matches exactly one row — the homonym pair below
 *  is deliberately kept out of this map so callers that iterate it stay
 *  single-match. */
export const MOCK_ROOMS = {
  main: 'Build Iroh Rooms MVP',
  workspace: 'Agent Workspace',
  review: 'Product Review',
  design: 'Design System',
  research: 'Research Lab',
} as const;

/** The name shared by the two homonymous fixture rooms (docs/room-workbench.md,
 *  decision 6). `roomItem(HOMONYM_ROOM)` matches BOTH rows, which is the point:
 *  only the short id shown on each row tells them apart. */
export const HOMONYM_ROOM = 'Bug Triage';

interface AppFixtures {
  /** True when this project's viewport is in the compact (phone) layout. */
  compact: boolean;
  /** Which of the three shells this project's viewport renders. */
  shell: Shell;
  app: AppDriver;
}

/** Small driver for flows every spec needs, breakpoint-aware so the same
 *  spec runs unchanged across all four viewport projects. */
export class AppDriver {
  constructor(
    readonly page: Page,
    readonly compact: boolean,
    readonly shell: Shell = compact ? 'compact' : 'wide',
  ) {}

  /** Load the app and wait for the shell (populated fixture) to be ready.
   *
   *  Boot restores the last room, so `/` settles on `/rooms/:id/activity` with
   *  Rooms pushed behind it. On compact that means the room pane, not the
   *  rooms list — so readiness is the room being open, which is the one signal
   *  every shell shares. Use `gotoRoomsList()` when the list itself is the
   *  subject. */
  async gotoPopulated(params: Record<string, string> = {}): Promise<void> {
    await this.goto(params);
    await expect(this.page).toHaveURL(/\/rooms\/[^/]+\/activity/);
    await expect(this.timeline).toBeVisible();
  }

  /** Load the app and land on the rooms list on every shell. */
  async gotoRoomsList(params: Record<string, string> = {}): Promise<void> {
    await this.gotoPopulated(params);
    await this.showRoomsList();
  }

  /** The shell the page is ACTUALLY in right now.
   *
   *  `compact`/`shell` come from the project's viewport, which is right for
   *  every spec that keeps it — but responsive.spec.ts resizes the page, and a
   *  driver that trusted the project would take the desktop branch at 320px
   *  and click a rail that isn't there. */
  currentShell(): Shell {
    return shellForWidth(this.page.viewportSize()?.width ?? 0);
  }

  /** Show the rooms list. On compact it is a pane of its own; on medium and
   *  wide the rail is always visible, so this is already true there. */
  async showRoomsList(): Promise<void> {
    if (this.currentShell() === 'compact' && !(await this.sidebar.isVisible())) {
      await this.navigate('destRooms');
    }
    await expect(this.roomItem(MOCK_ROOMS.main)).toBeVisible();
  }

  /** Load the app with the fresh-onboarding fixture (`?mock=fresh`). */
  async gotoFresh(): Promise<void> {
    await this.goto({ mock: 'fresh' });
    await expect(this.page.getByRole('heading', { name: en.onboardingIdentityTitle })).toBeVisible();
  }

  private async goto(params: Record<string, string>): Promise<void> {
    const search = new URLSearchParams(params).toString();
    await this.page.goto(search ? `/?${search}` : '/');
    // Refuse to drive anything but the VITE_MOCK=1 fixture build. With
    // reuseExistingServer enabled locally, attaching to a stray non-mock
    // server would at best fail every spec confusingly and at worst drive a
    // REAL daemon (the onboarding spec creates a real, irreversible
    // identity). main.tsx stamps the transport on <html> for exactly this.
    await expect(
      this.page.locator('html'),
      'the server on the harness port is not the VITE_MOCK=1 build — refusing to run against a non-mock transport',
    ).toHaveAttribute('data-jeliya-transport', /mock fixtures \(VITE_MOCK=1\)/);
  }

  /** A room row's select button in the rail. Homonymous rooms share a name
   *  (decision 6), so `disambig` — the short id the row shows next to that name —
   *  narrows to a single one. Unique-named rooms need only the name, so existing
   *  callers pass it alone and still get their one row.
   *
   *  Scoped to `.room-select` (issue #64): each row now also carries pin/archive
   *  buttons whose accessible names contain the room name, so a bare name match
   *  would resolve to three buttons per row. */
  roomItem(name: string, disambig?: string) {
    // `name` is fixture DATA (a mock room name), so it is a literal by design.
    // The landmark's accessible name is COPY — it is the Rooms destination
    // word, so it resolves through the catalog like every other label.
    const byName = this.page
      .getByRole('navigation', { name: en.destRooms })
      .getByRole('button', { name, exact: false })
      .and(this.page.locator('.room-select'));
    return disambig ? byName.filter({ hasText: disambig }) : byName;
  }

  /** A lifecycle filter chip ("All" / "Active" / "Left & removed"). Scoped to
   *  the filter group so it never collides with the same-named departed
   *  disclosure inside the room list. */
  filterChip(filter: RoomFilter) {
    return this.page
      .getByRole('group', { name: en.roomsFilterLegend })
      .getByRole('button', { name: roomFilterLabel(filter), exact: true });
  }

  /** Expand the collapsed "Left & removed" disclosure so a departed room's row
   *  is in the DOM (issue #64 sections). No-op if already open or absent. The
   *  disclosure lives inside the Rooms navigation — scoped so it is not confused
   *  with the identically-labelled filter chip. */
  async showDeparted(): Promise<void> {
    // Substring match rather than a regex: the label is catalog copy, and
    // building a RegExp from translated text would need escaping in every
    // locale (French copy carries « » and narrow no-break spaces).
    const toggle = this.page
      .getByRole('navigation', { name: en.destRooms })
      .getByRole('button', { name: en.roomsFilterDeparted, exact: false });
    if ((await toggle.count()) > 0 && (await toggle.getAttribute('aria-expanded')) === 'false') {
      await toggle.click();
    }
  }

  /** The single-pane containers (visibility differs per breakpoint). */
  get sidebar() {
    return this.page.locator('.sidebar');
  }
  get center() {
    return this.page.locator('.center');
  }
  get rightPanel() {
    return this.page.locator('.right-panel');
  }
  get timeline() {
    return this.page.locator('.timeline');
  }
  get composerTextarea() {
    return this.page.locator('.composer-bar textarea');
  }
  get tabBar() {
    return this.page.getByRole('navigation', { name: en.shellNavPrimaryMobile });
  }

  /** The room's nested navigation — visible under the room header, and inside
   *  the inspector on compact. Exactly one of the two is on screen at a time,
   *  so this stays unambiguous on every shell. */
  roomTab(dest: RoomDest) {
    // Not exact: a tab's accessible name absorbs its count badge ("Files5").
    // The five labels share no prefix, so a substring match stays unambiguous.
    return this.page.getByRole('tab', { name: roomDestLabel(dest), exact: false });
  }

  mobileTab(dest: GlobalDest) {
    return this.tabBar.getByRole('button', { name: globalDestLabel(dest), exact: true });
  }

  /** Open a room so its Activity is visible, whatever the shell. */
  async openRoom(name: string): Promise<void> {
    await this.showRoomsList();
    await this.roomItem(name).click();
    await expect(this.page.getByRole('heading', { level: 1, name })).toBeVisible();
    await expect(this.timeline).toBeVisible();
  }

  /** Compose and send a message the way this shell requires (#67 P20): desktop
   *  sends on Enter, compact inserts a newline there so the ➤ button is the
   *  explicit send. Specs that are about scroll/filter behavior, not the send
   *  gesture itself, use this so they run unchanged on every viewport. */
  async sendMessage(body: string): Promise<void> {
    // `body` is fixture DATA — a literal on purpose.
    await this.composerTextarea.fill(body);
    if (this.currentShell() === 'compact') {
      await this.page.getByRole('button', { name: en.composerSendMessage }).click();
    } else {
      await this.composerTextarea.press('Enter');
    }
  }

  /** Go to a room destination. Activity closes the inspector; a tool opens it —
   *  a column on wide, a drawer on medium, the whole pane on compact, and one
   *  navigation on all three. */
  async goToRoomDest(dest: RoomDest): Promise<void> {
    await this.roomTab(dest).click();
    // Compared as a KEY, never as the visible label — the branch is about which
    // destination this is, which must not change when the words do.
    if (destKey(ROOM_DEST_ALIAS, dest) === 'roomDestActivity') {
      await expect(this.timeline).toBeVisible();
    } else {
      await expect(this.rightPanel).toBeVisible();
    }
  }

  /** Navigate to a global destination (room rail / compact tab bar). */
  async navigate(dest: GlobalDest): Promise<void> {
    if (this.currentShell() === 'compact') {
      // Inside a room the bar gives way to the room's app bar; Back returns to
      // it, exactly as a user would have to.
      if (!(await this.tabBar.isVisible())) {
        await this.page.getByRole('button', { name: en.roomBackToRooms }).click();
      }
      await this.mobileTab(dest).click();
    } else {
      await this.page
        .getByRole('navigation', { name: en.shellNavPrimary, exact: true })
        .getByRole('button', { name: globalDestLabel(dest), exact: true })
        .click();
    }
  }

  /** Distance of the timeline scroller from its bottom edge, in px. */
  async timelineBottomOffset(): Promise<number> {
    return this.timeline.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight);
  }
}

export const test = base.extend<AppFixtures>({
  compact: [
    async ({ viewport }, use) => {
      await use((viewport?.width ?? 1280) <= COMPACT_MAX_WIDTH);
    },
    { auto: false },
  ],
  shell: [
    async ({ viewport }, use) => {
      await use(shellForWidth(viewport?.width ?? 1280));
    },
    { auto: false },
  ],
  app: async ({ page, compact, shell }, use, testInfo) => {
    // Console-error trail: collected for the whole test, attached on failure
    // alongside Playwright's screenshot + trace so a broken viewport ships
    // with everything needed to diagnose it.
    const consoleErrors: string[] = [];
    page.on('console', (message) => {
      if (message.type() === 'error') consoleErrors.push(message.text());
    });
    page.on('pageerror', (error) => consoleErrors.push(`pageerror: ${error.message}`));

    await use(new AppDriver(page, compact, shell));

    if (testInfo.status !== testInfo.expectedStatus) {
      const viewport = page.viewportSize();
      // The EXACT combination the failure happened under. A viewport alone is
      // not reproducible once the matrix has a locale and a text-scale
      // dimension — the reader would have to guess which cell failed
      // (issue #76). Read from the live page rather than the project config so
      // a test that overrides either one still reports the truth.
      const runtime = await page
        .evaluate(() => ({
          lang: document.documentElement.lang || '(unset)',
          textScale: getComputedStyle(document.documentElement).fontSize,
          reducedMotion: window.matchMedia('(prefers-reduced-motion: reduce)').matches,
        }))
        .catch(() => null);
      await testInfo.attach('failing-viewport', {
        body: [
          `${testInfo.project.name} (${viewport?.width}x${viewport?.height})`,
          `locale: ${runtime?.lang ?? '(page unavailable)'}`,
          `root font-size: ${runtime?.textScale ?? '(page unavailable)'}`,
          `prefers-reduced-motion: ${runtime?.reducedMotion ?? '(page unavailable)'}`,
        ].join('\n'),
        contentType: 'text/plain',
      });
      await testInfo.attach('console-errors', {
        body: consoleErrors.length > 0 ? consoleErrors.join('\n') : '(no console errors)',
        contentType: 'text/plain',
      });
    }
  },
});

export { expect };
