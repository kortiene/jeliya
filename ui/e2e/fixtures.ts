import { expect, test as base } from '@playwright/test';
import type { Page } from '@playwright/test';

/** The compact breakpoint from styles.css (`@media (max-width: 900px)`). */
export const COMPACT_MAX_WIDTH = 900;

/** Room names from the populated mock fixture (ui/src/lib/mock.ts). */
export const MOCK_ROOMS = {
  main: 'Build Iroh Rooms MVP',
  workspace: 'Agent Workspace',
  review: 'Product Review',
  design: 'Design System',
  research: 'Research Lab',
} as const;

interface AppFixtures {
  /** True when this project's viewport is in the compact (phone) layout. */
  compact: boolean;
  app: AppDriver;
}

/** Small driver for flows every spec needs, breakpoint-aware so the same
 *  spec runs unchanged across all four viewport projects. */
export class AppDriver {
  constructor(
    readonly page: Page,
    readonly compact: boolean,
  ) {}

  /** Load the app and wait for the shell (populated fixture) to be ready. */
  async gotoPopulated(params: Record<string, string> = {}): Promise<void> {
    await this.goto(params);
    // Ready = the room list has fixture rooms (boot + room.list resolved).
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

  roomItem(name: string) {
    return this.page
      .getByRole('navigation', { name: 'Rooms' })
      .getByRole('button', { name, exact: false });
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

  mobileTab(label: 'Rooms' | 'Agents' | 'Pipes' | 'Files' | 'Settings') {
    return this.tabBar.getByRole('button', { name: label });
  }

  /** Open a room so its chat pane is visible, whatever the breakpoint.
   *  On desktop all columns are visible; on compact this taps through the
   *  Rooms pane. */
  async openRoom(name: string): Promise<void> {
    if (this.compact) {
      // The Rooms pane is the compact landing view; get back to it first so
      // the room item is tappable from any pane.
      await this.mobileTab('Rooms').click();
    }
    await this.roomItem(name).click();
    await expect(this.page.getByRole('heading', { level: 1, name })).toBeVisible();
    await expect(this.timeline).toBeVisible();
  }

  /** Navigate to a primary destination (desktop left rail / mobile tab bar). */
  async navigate(label: 'Rooms' | 'Agents' | 'Pipes' | 'Files' | 'Settings'): Promise<void> {
    if (this.compact) {
      await this.mobileTab(label).click();
    } else {
      await this.page
        .getByRole('navigation', { name: 'Primary', exact: true })
        .getByRole('button', { name: label })
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
  app: async ({ page, compact }, use, testInfo) => {
    // Console-error trail: collected for the whole test, attached on failure
    // alongside Playwright's screenshot + trace so a broken viewport ships
    // with everything needed to diagnose it.
    const consoleErrors: string[] = [];
    page.on('console', (message) => {
      if (message.type() === 'error') consoleErrors.push(message.text());
    });
    page.on('pageerror', (error) => consoleErrors.push(`pageerror: ${error.message}`));

    await use(new AppDriver(page, compact));

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
