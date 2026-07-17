import { expect, test } from './fixtures';

// The top-level Agent Fleet dashboard.

test('shows the agent fleet with honest liveness', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Agents');

  const fleet = page.getByRole('region', { name: 'Agents fleet' });
  await expect(fleet).toBeVisible();
  await expect(fleet.getByRole('heading', { level: 1, name: 'Agents' })).toBeVisible();

  // Each fixture agent gets exactly one aggregated card, not one per room.
  const card = (name: string) => fleet.locator('.fleet-card', { hasText: name });
  for (const name of ['Backend Agent', 'QA Agent', 'Research Agent']) {
    await expect(card(name)).toHaveCount(1);
  }

  // The honesty rule this dashboard exists for (§1.2): a crashed runner whose
  // latest label is working-class but whose peer is gone must read Stale,
  // never Working — the mock builds exactly that fixture for Research Agent.
  // Backend has a connected peer and a fresh working status: really Working.
  await expect(card('Research Agent').locator('.live-pill')).toHaveText(/Stale/);
  // Working needs the auto-opened main room's live peer; the dashboard polls
  // agents.fleet every 4s, so allow one full cycle beyond the default.
  await expect(card('Backend Agent').locator('.live-pill')).toHaveText(/Working/, {
    timeout: 10_000,
  });
});

test('searching filters the fleet list', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Agents');

  const fleet = page.getByRole('region', { name: 'Agents fleet' });
  await fleet.getByLabel('Search agents').fill('Research');
  await expect(fleet.getByText('Research Agent').first()).toBeVisible();
  await expect(fleet.getByText('Backend Agent')).toHaveCount(0);
});
