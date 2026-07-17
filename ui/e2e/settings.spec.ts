import { expect, test } from './fixtures';

// The Settings pane (compact tab / desktop overlay).

test('shows identity, endpoint, daemon state, and diagnostics', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Settings');

  const settings = page.getByRole('region', { name: 'Settings' });
  await expect(settings).toBeVisible();
  await expect(settings.getByText('P2P Identity')).toBeVisible();
  // The real 64-hex identity id, not a placeholder.
  await expect(settings.locator('.settings-val').first()).toHaveText(/^[0-9a-f]{64}$/);
  // Honest daemon state: mock mode reports loopback + live connection state.
  await expect(settings.getByText('loopback · connected')).toBeVisible();
  await expect(settings.getByRole('button', { name: 'Copy diagnostics' })).toBeEnabled();
  await expect(settings.getByRole('button', { name: 'Report issue' })).toBeVisible();
});
