import { expect, test } from './fixtures';

// Deep links and tool routes to rooms the daemon cannot open must resolve to a
// visible recovery state, not a blank pane (docs/room-workbench.md, decision 2)
// — including when the route names a tool, where the naive derivation would
// open an empty inspector over a hidden recovery surface.

const MISSING = 'blake3:0000000000000000000000000000000000000000000000000000000000000000';

test('a tool route to a room not on this device shows the recovery surface, not an empty inspector', async ({
  app,
  page,
}) => {
  // The recovery UI lives in .center. A tool route (…/files) derives an
  // inspector pane, and compact CSS hides .center for it — so without the
  // guard the user lands in an empty tool with no way out, and file/pipe
  // actions would fire against a room that isn't open.
  await app.gotoPopulated();
  await page.goto(`/rooms/${encodeURIComponent(MISSING)}/files`);

  await expect(page.locator('.room-gone-title')).toBeVisible();
  await expect(page.locator('.center')).toBeVisible();
  await expect(app.rightPanel).toBeHidden();
  await expect(page.getByRole('button', { name: 'Back to Rooms' })).toBeVisible();
});

test('Back to Rooms escapes a tool route to a missing room', async ({ app, page }) => {
  await app.gotoPopulated();
  await page.goto(`/rooms/${encodeURIComponent(MISSING)}/pipes`);
  await expect(page.locator('.room-gone-title')).toBeVisible();

  await page.getByRole('button', { name: 'Back to Rooms' }).click();
  await expect(page).toHaveURL(/\/rooms$/);
  await expect(app.sidebar).toBeVisible();
});
