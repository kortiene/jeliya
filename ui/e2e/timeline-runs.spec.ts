import { expect, test } from './fixtures';

// Issue #65: repeated agent status updates fold into one expandable RUN, an
// activity filter isolates categories WITHOUT deleting history, pending
// messages are exempt from the filter, and the floating counter tells the truth
// about a mixed batch ("N new activity", not "N new messages").
//
// The main-room fixture (ui/src/lib/mock.ts) carries the one foldable streak:
// BACKEND posts three consecutive statuses at 9:05–9:09 with nothing between
// them. Every OTHER agent_status in the room is separated by a file/message/
// different sender, so the same view exercises both the folded and the
// standalone cases against real signed data. The whole suite runs under
// prefers-reduced-motion: reduce (playwright.config.ts), so the expand reveal
// is exercised in its instant, no-animation form.

test('a burst of agent statuses folds into one expandable run', async ({ app }) => {
  await app.gotoPopulated();

  // The burst shows as ONE summary: the LATEST signed status plus honest run
  // evidence ("3 updates" + the real time span) and a disclosure.
  const toggle = app.timeline.getByRole('button', { name: 'Show 3 updates' });
  await expect(toggle).toHaveCount(1);
  await expect(app.timeline.getByText('Invite tickets minting and redeeming end-to-end.')).toBeVisible();
  await expect(app.timeline.locator('.agent-run-count').filter({ hasText: '3 updates' })).toBeVisible();
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');

  // Collapsed, the earlier updates are folded out of the DOM — folded, never
  // lost.
  await expect(app.timeline.getByText('Scaffolding room invite flow and peer discovery.')).toHaveCount(0);
  await expect(app.timeline.getByText('Peer discovery handshakes verified across 3 relays.')).toHaveCount(0);

  // Expanding reveals every original update in chronological order; history is
  // preserved and inspectable.
  await toggle.click();
  await expect(app.timeline.getByText('Scaffolding room invite flow and peer discovery.')).toBeVisible();
  await expect(app.timeline.getByText('Peer discovery handshakes verified across 3 relays.')).toBeVisible();
  const hide = app.timeline.getByRole('button', { name: 'Hide' });
  await expect(hide).toHaveAttribute('aria-expanded', 'true');

  // Collapsing folds them away again, and the run is still expandable — nothing
  // was consumed by looking at it.
  await hide.click();
  await expect(app.timeline.getByText('Scaffolding room invite flow and peer discovery.')).toHaveCount(0);
  await expect(app.timeline.getByRole('button', { name: 'Show 3 updates' })).toBeVisible();
});

test('an activity filter isolates agent runs and hides conversation without losing it', async ({ app }) => {
  await app.gotoPopulated();

  const conversation = app.timeline.getByText('Kicked off the rooms protocol spec', { exact: false });
  const agentRuns = app.page
    .getByRole('group', { name: 'Filter activity' })
    .getByRole('button', { name: 'Agent runs', exact: true });

  // Baseline: a human message and an agent run are both on screen.
  await expect(conversation).toHaveCount(1);
  await expect(app.timeline.getByRole('button', { name: 'Show 3 updates' })).toBeVisible();

  // Isolate agent runs — the conversation drops out of the VIEW, the run stays.
  await agentRuns.click();
  await expect(agentRuns).toHaveAttribute('aria-pressed', 'true');
  await expect(conversation).toHaveCount(0);
  await expect(app.timeline.getByRole('button', { name: 'Show 3 updates' })).toBeVisible();

  // Toggling the filter off restores the conversation verbatim: the filter
  // SEPARATES, it never deletes.
  await agentRuns.click();
  await expect(agentRuns).toHaveAttribute('aria-pressed', 'false');
  await expect(conversation).toHaveCount(1);
});

test('pending messages are never filtered out', async ({ app }) => {
  // A failed send leaves a STABLE pending card (with Retry) rather than one that
  // resolves away, so we can prove a filter never touches it.
  await app.gotoPopulated({ mock_fail: 'message.send:1' });

  await app.sendMessage('ping that must survive filtering');
  const pending = app.timeline.getByText('ping that must survive filtering');
  await expect(pending).toBeVisible();
  await expect(app.timeline.getByRole('button', { name: 'Retry' })).toBeVisible();

  // Filter to a category the message does NOT belong to…
  await app.page
    .getByRole('group', { name: 'Filter activity' })
    .getByRole('button', { name: 'Agent runs', exact: true })
    .click();

  // …the pending message is exempt and stays on screen — retry stays reachable.
  await expect(pending).toBeVisible();
  await expect(app.timeline.getByRole('button', { name: 'Retry' })).toBeVisible();
});

test('the counter reads "new activity" when a non-message is among the new items', async ({ app, viewport }) => {
  // The counter WORDING is viewport-independent, and the only deterministic
  // non-message "new" item is the mock's live agent_status ~6 s after open — so
  // this runs on one shell rather than holding four workers idle for that wait.
  test.skip((viewport?.width ?? 0) !== 1440, 'shell-independent wording; one shell is enough');
  await app.gotoPopulated();

  // Read from the top so incoming events are counted, not silently auto-followed.
  await expect(app.timeline.getByText('Sync convergence suite running (14/24 green).')).toBeVisible();
  await app.timeline.evaluate((el) => {
    el.scrollTop = 0;
  });
  await expect.poll(() => app.timeline.evaluate((el) => el.scrollTop)).toBe(0);

  // The mock posts a live agent_status (a non-message) a few seconds after open.
  // Because the new batch is not all-messages, the pill is worded as mass-noun
  // activity — never "N new messages".
  const pill = app.page.locator('.new-messages');
  await expect(pill).toContainText('new activity', { timeout: 15_000 });
  await expect(pill).not.toContainText('message');
});
