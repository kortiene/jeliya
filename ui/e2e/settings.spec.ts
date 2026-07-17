import { expect, test, MOCK_ROOMS } from './fixtures';

// Settings — one of the three global destinations (docs/room-workbench.md,
// decision 1), at `/settings`. It is a pane of its own on compact and an
// overlay over the workspace on medium and wide.

test('shows identity, endpoint, daemon state, and diagnostics', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Settings');

  // The route is the navigation state (decision 2), so the destination being
  // reached is a fact about the URL, not only about what happens to be painted.
  await expect(page).toHaveURL(/\/settings$/);

  const settings = page.getByRole('region', { name: 'Settings' });
  await expect(settings).toBeVisible();
  await expect(settings.getByText('P2P Identity')).toBeVisible();
  await expect(settings.getByText('Endpoint')).toBeVisible();
  // The real 64-hex identity and endpoint ids, not the '-' the panel renders
  // before daemon.status answers: a placeholder shown as a value would be the
  // screen asserting a fact it does not have (decision 5).
  await expect(settings.locator('.settings-val').first()).toHaveText(/^[0-9a-f]{64}$/);
  await expect(settings.locator('.settings-val').nth(1)).toHaveText(/^[0-9a-f]{64}$/);
  // Honest daemon state: mock mode reports loopback + live connection state.
  await expect(settings.getByText('loopback · connected')).toBeVisible();
  await expect(settings.getByRole('button', { name: 'Copy diagnostics' })).toBeEnabled();
  await expect(settings.getByRole('button', { name: 'Report issue' })).toBeVisible();
});

test('Settings keeps a way back out on every shell', async ({ app, page, compact }) => {
  // Boot lands inside a room, where the compact bottom bar gives way to the
  // room's app bar. Settings has no Back of its own, so the bar has to return
  // here or a phone user is stranded on a destination they cannot leave; the
  // rail plays the same part on medium and wide.
  await app.gotoPopulated();
  await app.navigate('Settings');
  await expect(page.getByRole('region', { name: 'Settings' })).toBeVisible();

  if (compact) await expect(app.tabBar).toBeVisible();
  else await expect(app.sidebar).toBeVisible();

  await app.navigate('Rooms');
  await expect(page).toHaveURL(/\/rooms$/);
  await expect(app.roomItem(MOCK_ROOMS.main)).toBeVisible();
});
