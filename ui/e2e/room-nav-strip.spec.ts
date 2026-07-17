import { expect, test } from './fixtures';

// The room's tab strip carries hard-coded ids (`room-tab-*`) for its
// roving-tabindex keyboard handler. Exactly one strip may be live at a time —
// two would duplicate the ids, and getElementById would move focus into the
// hidden copy (docs/room-workbench.md, decision 3).

test.describe('exactly one room-nav strip is ever live', () => {
  for (const width of [920, 1280] as const) {
    test(`${width}px keeps one #room-tab-people while the inspector is open`, async ({ app, page }) => {
      await page.setViewportSize({ width, height: 800 });
      await app.gotoPopulated();

      // Closed: only the workspace strip.
      await expect(page.locator('#room-tab-people')).toHaveCount(1);

      // Open a tool — medium floats a drawer, wide adds a column, and both
      // carry their own strip. The workspace copy must stand down.
      await app.goToRoomDest('Files');
      await expect(page.locator('#room-tab-people')).toHaveCount(1);
      await expect(page.locator('#room-tab-files')).toHaveCount(1);
    });
  }
});

test('keyboard arrows move focus between visible tabs, never into a hidden copy', async ({ app, page }) => {
  await page.setViewportSize({ width: 920, height: 800 });
  await app.gotoPopulated();
  await app.goToRoomDest('People');

  // Focus the live People tab and arrow to the next: focus must land on a
  // visible element (the drawer's own strip), not a display:none duplicate.
  await page.locator('#room-tab-people').focus();
  await page.keyboard.press('ArrowRight');

  const focused = page.locator('*:focus');
  await expect(focused).toBeVisible();
  await expect(focused).toHaveAttribute('role', 'tab');
});
