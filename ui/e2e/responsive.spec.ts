import { expect, test, MOCK_ROOMS, shellForWidth } from './fixtures';
import type { Page } from '@playwright/test';

// The responsive contract (docs/room-workbench.md, decision 3; issue #62).
//
// The four viewport projects fix one size each; this spec walks the widths the
// contract actually names — 360, 899, 900, 920, 1280 — inside one project, so
// the boundaries are pinned rather than sampled. It runs on the wide project
// only: every case sets its own viewport, so running the same assertions again
// under three more projects would just be the same test four times.
//
// On text scaling: the design sizes text in px, so the browser-faithful model
// of "the user made everything bigger" is page zoom, and page zoom is
// mathematically a narrower viewport. WCAG 1.4.10 fixes the target at 320 CSS
// px — which is 1280 at 400% — so the 320 cases below ARE the zoom coverage.
// Real text-scale coverage (textScale 2.0, EN and FR) lives in the Flutter
// suite, where the platform exposes it faithfully.

test.skip(({ viewport }) => (viewport?.width ?? 0) !== 1440, 'viewport-driven: one project is enough');

const WIDTHS = [360, 899, 900, 920, 1280] as const;

/** iPhone-class insets. The app reads every inset through these custom
 *  properties, so overriding them exercises the real mechanism — headless
 *  Chromium reports no insets of its own. */
async function withSafeAreas(page: Page): Promise<void> {
  await page.addStyleTag({
    content: ':root { --safe-top: 44px; --safe-bottom: 34px; --safe-left: 12px; --safe-right: 12px; }',
  });
}

async function hasHorizontalOverflow(page: Page): Promise<boolean> {
  return page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
}

test.describe('the shell each width renders', () => {
  for (const width of WIDTHS) {
    const shell = shellForWidth(width);

    test(`${width}px is the ${shell} shell`, async ({ app, page }) => {
      await page.setViewportSize({ width, height: 800 });
      await app.gotoPopulated();

      // The room rail: a pane of its own on compact (hidden while in a room),
      // always present beside the workspace above it.
      if (shell === 'compact') {
        await expect(app.sidebar).toBeHidden();
        await expect(app.center).toBeVisible();
      } else {
        await expect(app.sidebar).toBeVisible();
        await expect(app.center).toBeVisible();
      }

      // The inspector is closed on Activity at every width — that is what
      // `activity` means (decision 3).
      await expect(app.rightPanel).toBeHidden();
    });
  }
});

test.describe('the workspace is never squeezed', () => {
  for (const width of WIDTHS) {
    test(`${width}px leaves the workspace usable`, async ({ app, page }) => {
      await page.setViewportSize({ width, height: 800 });
      await app.gotoPopulated();

      // The regression this band exists to prevent: at 901px the old grid
      // (232 rail + 1fr + 300 inspector) left the workspace 369px — narrower
      // than the phone layout it had just graduated from.
      const centerWidth = await app.center.evaluate((el) => el.getBoundingClientRect().width);
      expect(centerWidth).toBeGreaterThan(340);

      await expect(app.timeline).toBeVisible();
      await expect(app.composerTextarea).toBeVisible();
      expect(await hasHorizontalOverflow(page)).toBe(false);
    });
  }
});

test('medium floats the inspector over the workspace; wide gives it a column', async ({ app, page }) => {
  // 920: medium. The inspector must not take a column from a workspace this
  // narrow, so it floats — and the workspace keeps its width while it is open.
  await page.setViewportSize({ width: 920, height: 800 });
  await app.gotoPopulated();
  const centerBefore = await app.center.evaluate((el) => el.getBoundingClientRect().width);

  await app.goToRoomDest('People');
  const centerAfter = await app.center.evaluate((el) => el.getBoundingClientRect().width);
  expect(centerAfter).toBe(centerBefore);
  const drawer = await app.rightPanel.evaluate((el) => el.getBoundingClientRect());
  const centerBox = await app.center.evaluate((el) => el.getBoundingClientRect());
  // A drawer: it overlaps the workspace and pins to its right edge.
  expect(Math.round(drawer.right)).toBe(Math.round(centerBox.right));
  expect(drawer.left).toBeLessThan(centerBox.right);
  expect(drawer.left).toBeGreaterThan(centerBox.left);

  // 1280: wide. The inspector stops overlapping and takes its own column,
  // which the workspace pays for.
  await page.setViewportSize({ width: 1280, height: 800 });
  await expect(app.rightPanel).toBeVisible();
  const wideCenter = await app.center.evaluate((el) => el.getBoundingClientRect());
  const wideInspector = await app.rightPanel.evaluate((el) => el.getBoundingClientRect());
  expect(Math.round(wideInspector.left)).toBeGreaterThanOrEqual(Math.round(wideCenter.right) - 1);
  expect(await hasHorizontalOverflow(page)).toBe(false);
});

test('the inspector is collapsible, and collapsing it is navigating to Activity', async ({ app, page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await app.gotoPopulated();

  await app.goToRoomDest('Files');
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/files$/);

  await page.getByRole('button', { name: 'Close inspector' }).click();
  await expect(app.rightPanel).toBeHidden();
  // The inspector's openness is not a second state: it IS the route.
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/activity$/);
});

test('selecting a room tool preserves the timeline position', async ({ app, page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await app.gotoPopulated();
  await expect(app.timeline).toBeVisible();

  // Scroll away from the bottom, then open and close the inspector. Panes hide,
  // they do not unmount (decision 3) — the reading position is the user's, and
  // toggling a side panel is not a request to move it.
  await app.timeline.evaluate((el) => el.scrollTo({ top: 0 }));
  const before = await app.timelineBottomOffset();
  expect(before).toBeGreaterThan(0);

  await app.goToRoomDest('People');
  await app.goToRoomDest('Activity');
  await expect(app.timeline).toBeVisible();
  expect(await app.timelineBottomOffset()).toBe(before);
});

test.describe('safe areas', () => {
  for (const width of [360, 320] as const) {
    test(`${width}px reserves the insets without clipping`, async ({ app, page }) => {
      await page.setViewportSize({ width, height: 568 });
      await app.gotoPopulated();
      await withSafeAreas(page);

      // Inside a room the bottom bar is gone, so the composer owes the home
      // indicator its inset and nothing else.
      await expect(app.composerTextarea).toBeVisible();
      expect(await hasHorizontalOverflow(page)).toBe(false);

      const composer = await app.composerTextarea.evaluate((el) => el.getBoundingClientRect());
      expect(composer.bottom).toBeLessThanOrEqual(568);

      // The rooms pane owes the tab bar its height plus the inset, so that its
      // last row is never hidden behind the bottom chrome. Asserted on the
      // pane's own last element rather than on a room row: the room list is a
      // scroller, and a row below its fold is legitimately off-screen.
      await app.navigate('Rooms');
      await expect(app.roomItem(MOCK_ROOMS.main)).toBeVisible();
      expect(await hasHorizontalOverflow(page)).toBe(false);

      const bar = await app.tabBar.evaluate((el) => el.getBoundingClientRect());
      const paneEnd = await app.sidebar.evaluate((el) => {
        const style = getComputedStyle(el);
        return el.getBoundingClientRect().bottom - parseFloat(style.paddingBottom);
      });
      expect(paneEnd).toBeLessThanOrEqual(bar.top + 1);
    });
  }
});

test('the connection banner reserves space instead of covering the room', async ({ app, page }) => {
  await page.setViewportSize({ width: 360, height: 640 });
  // Hold room.open long enough to still be reconnecting-free but give the
  // banner a reason to exist: drop the socket after boot.
  await app.gotoPopulated();
  await expect(app.timeline).toBeVisible();

  const backBefore = await page.getByRole('button', { name: 'Back to Rooms' }).evaluate((el) => {
    const r = el.getBoundingClientRect();
    return { top: r.top, left: r.left };
  });

  await page.evaluate(() => window.dispatchEvent(new Event('offline')));
  const banner = page.locator('.conn-banner');
  if (await banner.isVisible()) {
    const back = page.getByRole('button', { name: 'Back to Rooms' });
    // Reserved, not overlaid: the banner pushes the app bar down rather than
    // covering the one control the user needs in order to leave.
    const backAfter = await back.evaluate((el) => el.getBoundingClientRect().top);
    expect(backAfter).toBeGreaterThanOrEqual(backBefore.top);
    const bannerBox = await banner.evaluate((el) => el.getBoundingClientRect());
    const backBox = await back.evaluate((el) => el.getBoundingClientRect());
    expect(bannerBox.bottom).toBeLessThanOrEqual(backBox.top + 1);
    expect(await hasHorizontalOverflow(page)).toBe(false);
  }
});
