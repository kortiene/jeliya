import { expect, test as base } from '@playwright/test';
import type { Page } from '@playwright/test';

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

/** The global destinations — the only three (docs/room-workbench.md). */
export type GlobalDest = 'Rooms' | 'Agent Fleet' | 'Settings';

/** The room destinations, as they are labelled. */
export type RoomDestLabel = 'Activity' | 'People' | 'Agents & Runs' | 'Files' | 'Pipes';

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
      await this.navigate('Rooms');
    }
    await expect(this.roomItem(MOCK_ROOMS.main)).toBeVisible();
  }

  /** Load the app with the fresh-onboarding fixture (`?mock=fresh`). */
  async gotoFresh(): Promise<void> {
    await this.goto({ mock: 'fresh' });
    await expect(this.page.getByRole('heading', { name: 'Create your identity' })).toBeVisible();
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

  /** A room row in the rail. Homonymous rooms share a name (decision 6), so
   *  `disambig` — the short id the row shows next to that name — narrows to a
   *  single one. Unique-named rooms need only the name, so existing callers
   *  pass it alone and still get their one row. */
  roomItem(name: string, disambig?: string) {
    const byName = this.page
      .getByRole('navigation', { name: 'Rooms' })
      .getByRole('button', { name, exact: false });
    return disambig ? byName.filter({ hasText: disambig }) : byName;
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
    return this.page.getByRole('navigation', { name: 'Primary (mobile)' });
  }

  /** The room's nested navigation — visible under the room header, and inside
   *  the inspector on compact. Exactly one of the two is on screen at a time,
   *  so this stays unambiguous on every shell. */
  roomTab(label: RoomDestLabel) {
    // Not exact: a tab's accessible name absorbs its count badge ("Files5").
    // The five labels share no prefix, so a substring match stays unambiguous.
    return this.page.getByRole('tab', { name: label, exact: false });
  }

  mobileTab(label: GlobalDest) {
    return this.tabBar.getByRole('button', { name: label, exact: true });
  }

  /** Open a room so its Activity is visible, whatever the shell. */
  async openRoom(name: string): Promise<void> {
    await this.showRoomsList();
    await this.roomItem(name).click();
    await expect(this.page.getByRole('heading', { level: 1, name })).toBeVisible();
    await expect(this.timeline).toBeVisible();
  }

  /** Go to a room destination. Activity closes the inspector; a tool opens it —
   *  a column on wide, a drawer on medium, the whole pane on compact, and one
   *  navigation on all three. */
  async goToRoomDest(label: RoomDestLabel): Promise<void> {
    await this.roomTab(label).click();
    if (label === 'Activity') {
      await expect(this.timeline).toBeVisible();
    } else {
      await expect(this.rightPanel).toBeVisible();
    }
  }

  /** Navigate to a global destination (room rail / compact tab bar). */
  async navigate(label: GlobalDest): Promise<void> {
    if (this.currentShell() === 'compact') {
      // Inside a room the bar gives way to the room's app bar; Back returns to
      // it, exactly as a user would have to.
      if (!(await this.tabBar.isVisible())) {
        await this.page.getByRole('button', { name: 'Back to Rooms' }).click();
      }
      await this.mobileTab(label).click();
    } else {
      await this.page
        .getByRole('navigation', { name: 'Primary', exact: true })
        .getByRole('button', { name: label, exact: true })
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
      await testInfo.attach('failing-viewport', {
        body: `${testInfo.project.name} (${viewport?.width}x${viewport?.height})`,
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
