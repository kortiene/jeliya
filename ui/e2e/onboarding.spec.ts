import { expect, test } from './fixtures';

// Fresh-daemon onboarding (`?mock=fresh`): identity creation, then the
// create-or-join rooms step, through to the ready shell.

test('creates an identity and a first room', async ({ app, page }) => {
  await app.gotoFresh();

  await page.getByRole('button', { name: 'Create identity' }).click();

  // Rooms step: both cards and the identity handoff panel are shown.
  await expect(page.getByRole('heading', { name: 'Create a room' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Join with a ticket' })).toBeVisible();
  await expect(page.getByText('Your identity id')).toBeVisible();

  await page.getByLabel('Room name').fill('Harness Test Room');
  await page.getByRole('button', { name: 'Create room' }).click();

  // Ready shell: the new room is in the rooms list on every breakpoint
  // (compact lands on the Rooms pane; desktop shows the sidebar).
  await expect(app.roomItem('Harness Test Room')).toBeVisible();

  // And it opens into a working chat pane.
  await app.openRoom('Harness Test Room');
  await expect(app.composerTextarea).toBeVisible();
});

test('surfaces a join failure honestly', async ({ app, page }) => {
  await app.gotoFresh();
  await page.getByRole('button', { name: 'Create identity' }).click();
  await expect(page.getByRole('heading', { name: 'Join with a ticket' })).toBeVisible();

  // A well-formed ticket nobody minted must fail with a real error — the
  // mock enforces the same contract as the daemon (no fabricated rooms).
  await page.getByLabel('Ticket').fill(`roomtkt1${'a'.repeat(90)}`);
  await page.getByRole('button', { name: 'Join room' }).click();

  await expect(page.locator('.error-note')).toBeVisible();
  // Still on the rooms step — no comforting fake transition.
  await expect(page.getByRole('heading', { name: 'Create a room' })).toBeVisible();
});
