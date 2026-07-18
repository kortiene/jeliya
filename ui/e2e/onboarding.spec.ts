import { expect, test } from './fixtures';

// Fresh-daemon onboarding (`?mock=fresh`): identity creation, then the
// create-or-join rooms step, through to the ready shell.
//
// This spec drives the one fixture with no rooms, so it cannot use the
// driver's `showRoomsList`/`openRoom` — both assert a room from the populated
// fixture. It reaches the rooms list through `navigate('Rooms')`, which is
// shell-agnostic and asserts nothing about which rooms exist.

test('creates an identity and a first room', async ({ app, page }) => {
  await app.gotoFresh();

  await page.getByRole('button', { name: 'Create identity' }).click();

  // Rooms step: both cards and the identity handoff panel are shown.
  await expect(page.getByRole('heading', { name: 'Create a room' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Join with a ticket' })).toBeVisible();
  await expect(page.getByText('Your identity id')).toBeVisible();

  await page.getByLabel('Room name').fill('Harness Test Room');
  await page.getByRole('button', { name: 'Create room' }).click();

  // The ready shell restores the only room there is, so a first-run user
  // lands *inside* the room they just named — on every shell, compact
  // included — instead of on a rooms list or an empty "Select a room" pane.
  // Naming a room is the act of choosing it; making the user pick it again
  // would be the shell forgetting what it was just told.
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/activity/);
  await expect(page.getByRole('heading', { level: 1, name: 'Harness Test Room' })).toBeVisible();
  await expect(app.timeline).toBeVisible();
  await expect(app.composerTextarea).toBeVisible();

  // And the room is really in the rooms list: landing in a room proves the
  // client navigated, not that `room.create` produced a room the list can
  // hand back (issue #54 — an action must land on a visible surface).
  await app.navigate('Rooms');
  await expect(app.roomItem('Harness Test Room')).toBeVisible();
});

test('sets a device-local self label during onboarding', async ({ app, page }) => {
  await app.gotoFresh();
  await page.getByRole('button', { name: 'Create identity' }).click();
  await expect(page.getByText('Your identity id')).toBeVisible();

  // The optional device label sits with the identity handoff; naming yourself
  // here carries straight into the app.
  await page.getByLabel('Your name on this device').fill('Ada');
  await page.getByLabel('Room name').fill('Ada Room');
  await page.getByRole('button', { name: 'Create room' }).click();
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/activity/);

  // Self is identified by the friendly label + the distinct "this device"
  // marker in the roster of the room just created.
  await app.goToRoomDest('People');
  await expect(app.rightPanel.locator('.member-row', { hasText: 'this device' })).toContainText('Ada');

  // And it persisted to Settings, where it can be changed later. (Close the
  // inspector first so the route back to Rooms is available on every shell.)
  await app.goToRoomDest('Activity');
  await app.navigate('Settings');
  await expect(
    page.getByRole('region', { name: 'Settings' }).getByLabel('Your name on this device'),
  ).toHaveValue('Ada');
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
  // Still on the rooms step — no comforting fake transition. A failed join
  // must not advance onboarding, and must not route into a room that the
  // daemon never admitted this identity to.
  await expect(page.getByRole('heading', { name: 'Create a room' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/rooms\/[^/]+/);
});
