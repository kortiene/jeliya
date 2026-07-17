import { expect, test } from './fixtures';

// The Agent Fleet dashboard — one of the three global destinations
// (docs/room-workbench.md, decision 1). It answers "are my agents alive,
// anywhere"; a room's "Agents & Runs" tab answers "what has run here". Two
// destinations, two names, two scopes — this spec drives the global one, so it
// always arrives through global navigation, never through a room's tab strip.

test('shows the agent fleet with honest liveness', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Agent Fleet');

  // The route is the navigation state (decision 2), so arriving at a global
  // destination means standing on its route. Worth pinning here because the
  // compact path to it is indirect: boot lands inside the restored room, where
  // the bottom bar gives way to the room's app bar, and the bar only comes back
  // once Back to Rooms has left the room.
  await expect(page).toHaveURL(/\/fleet$/);

  const fleet = page.getByRole('region', { name: 'Agent Fleet' });
  await expect(fleet).toBeVisible();
  await expect(fleet.getByRole('heading', { level: 1, name: 'Agent Fleet' })).toBeVisible();

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
  // Working needs a live peer in an OPEN room, which the room restored at boot
  // supplies: leaving that room for a global destination stops rendering it, it
  // does not close its session — so the evidence behind Working outlives the
  // navigation on every shell, including the compact Back to Rooms detour. The
  // dashboard polls agents.fleet every 4s, so allow one full cycle beyond the
  // default.
  await expect(card('Backend Agent').locator('.live-pill')).toHaveText(/Working/, {
    timeout: 10_000,
  });
});

test('searching filters the fleet list', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Agent Fleet');

  const fleet = page.getByRole('region', { name: 'Agent Fleet' });
  await fleet.getByLabel('Search agents').fill('Research');
  await expect(fleet.getByText('Research Agent').first()).toBeVisible();
  await expect(fleet.getByText('Backend Agent')).toHaveCount(0);
});

test('needs attention surfaces the widened closed set before the aggregate tiles', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Agent Fleet');

  const fleet = page.getByRole('region', { name: 'Agent Fleet' });
  const attention = fleet.locator('.fleet-attention');
  await expect(attention).toBeVisible();

  // The exact gap #69 fixes: a FAILED agent (red tone) — silently dropped by
  // the old blue-only filter — now surfaces, and so does a stale agent.
  const row = (name: string) => attention.locator('.attention-row', { hasText: name });
  await expect(row('Deploy Agent').locator('.attention-reason')).toHaveText(/Failed/);
  await expect(row('Research Agent').locator('.attention-reason')).toHaveText(/Stale/);

  // Actionable agents appear BEFORE the aggregate totals (layout-independent
  // DOM-order check).
  const attentionBeforeStats = await fleet.evaluate((root) => {
    const a = root.querySelector('.fleet-attention');
    const s = root.querySelector('.fleet-stats');
    if (!a || !s) return false;
    return (a.compareDocumentPosition(s) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
  });
  expect(attentionBeforeStats).toBe(true);
});

test('metric reads "Agents working now" and a stale status is qualified', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Agent Fleet');

  const fleet = page.getByRole('region', { name: 'Agent Fleet' });

  // "Running tasks" was an inferred count the daemon cannot prove; it is gone on
  // every shell. The KPI tiles are a desktop affordance (the phone layout drops
  // the row deliberately), so the renamed metric is asserted where it renders.
  await expect(fleet.getByText('Running tasks')).toHaveCount(0);
  if (!app.compact) {
    await expect(fleet.getByText('Agents working now')).toBeVisible();
  }

  // A stale agent's last label is shown past-tense on every shell, never as a
  // bare live status (the Stale-pill-beside-"Working"-chip contradiction). The
  // card polls in, so allow a full cycle.
  const research = fleet.locator('.fleet-card', { hasText: 'Research Agent' });
  await expect(research.locator('.chip-label')).toHaveText(/Last: Working/, { timeout: 10_000 });
});
