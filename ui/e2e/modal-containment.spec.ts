import { expect, test, MOCK_ROOMS, AppDriver } from './fixtures';
import type { Page } from '@playwright/test';

// Issue #56: create/join/leave dialogs must contain their in-flight
// operation — no dismissal path (Escape, backdrop, ✕, re-submit) may hide a
// pending non-cancellable request whose result would later mutate state —
// and destructive dialogs must never give the destructive action first focus.

const TICKET_SUFFIX = 'e2econtainmentticket00000000000000000000';

function modal(page: Page) {
  return page.getByRole('dialog');
}

/** Leave is published from the room's People roster — the room tool that names
 *  the membership it ends (docs/room-workbench.md, decision 1). One tab click
 *  reaches it from Activity on every shell, so the dialog is opened the same
 *  way a user opens it. */
async function openLeaveDialog(app: AppDriver, page: Page, room: string): Promise<void> {
  await app.openRoom(room);
  await app.goToRoomDest('People');
  await app.rightPanel.getByRole('button', { name: 'Leave', exact: true }).click();
  await expect(modal(page)).toBeVisible();
}

/** Back to the rooms list from a room tool, the way a user walks there:
 *  Activity closes the tool, then Rooms. On medium and wide the rail never
 *  left the screen, so only the first step moves anything. */
async function returnToRoomsList(app: AppDriver): Promise<void> {
  await app.goToRoomDest('Activity');
  await app.showRoomsList();
}

test('create: success creates exactly one room and navigates once', async ({ app, page }) => {
  await app.gotoRoomsList();
  await page.locator('button.create-room:not(.join-room)').click();

  await modal(page).getByLabel('Room name').fill('Containment Test Room');
  await modal(page).getByRole('button', { name: 'Create room' }).click();

  await expect(modal(page)).toHaveCount(0);
  // The create landed once: the route names the new room, on every shell.
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/activity/);
  await expect(page.getByRole('heading', { level: 1, name: 'Containment Test Room' })).toBeVisible();
  // Then the rail — a pane of its own on compact, so reading the row means
  // stepping back to it. Exactly one room of that name exists (no duplicate
  // request fired), and the session the create opened is reported as the
  // session it is (docs/room-workbench.md, decision 4).
  await app.showRoomsList();
  await expect(app.roomItem('Containment Test Room')).toHaveCount(1);
  await expect(app.roomItem('Containment Test Room')).toContainText('Open');
});

test('create: a pending request cannot be dismissed or duplicated', async ({ app, page }) => {
  await app.gotoRoomsList({ mock_delay: 'room.create:1500' });
  await page.locator('button.create-room:not(.join-room)').click();

  await modal(page).getByLabel('Room name').fill('Pending Room');
  const submit = modal(page).getByRole('button', { name: 'Create room' });
  await submit.click();

  // In flight: submit reflects it and every dismissal path is contained.
  await expect(modal(page).getByRole('button', { name: 'Creating…' })).toBeDisabled();
  await page.keyboard.press('Escape');
  await expect(modal(page)).toBeVisible();
  await page.locator('.modal-backdrop').click({ position: { x: 5, y: 5 } });
  await expect(modal(page)).toBeVisible();
  await expect(modal(page).getByRole('button', { name: 'Close' })).toBeDisabled();

  // Success still lands exactly once.
  await expect(modal(page)).toHaveCount(0, { timeout: 10_000 });
  await app.showRoomsList();
  await expect(app.roomItem('Pending Room')).toHaveCount(1);
});

test('create: failure keeps an actionable error in the dialog', async ({ app, page }) => {
  await app.gotoRoomsList({ mock_fail: 'room.create:1' });
  await page.locator('button.create-room:not(.join-room)').click();

  await modal(page).getByLabel('Room name').fill('Doomed Room');
  await modal(page).getByRole('button', { name: 'Create room' }).click();

  // The failure surfaces inside the dialog and interaction is restored.
  await expect(modal(page).locator('.error-note')).toBeVisible();
  await expect(modal(page).getByRole('button', { name: 'Create room' })).toBeEnabled();
  await expect(app.roomItem('Doomed Room')).toHaveCount(0);

  // No longer busy: Escape dismisses again.
  await page.keyboard.press('Escape');
  await expect(modal(page)).toHaveCount(0);
});

test('join: a pending join is contained, then applies its transition once', async ({
  app,
  page,
}) => {
  await app.gotoRoomsList({ mock_ticket: TICKET_SUFFIX, mock_delay: 'room.join:1500' });
  await page.getByRole('button', { name: 'Join with a ticket' }).click();

  await modal(page).getByLabel('Ticket').fill(`roomtkt1${TICKET_SUFFIX}`);
  await modal(page).getByRole('button', { name: 'Join room' }).click();

  await expect(modal(page).getByRole('button', { name: 'Joining…' })).toBeDisabled();
  await page.keyboard.press('Escape');
  await expect(modal(page)).toBeVisible();
  await page.locator('.modal-backdrop').click({ position: { x: 5, y: 5 } });
  await expect(modal(page)).toBeVisible();
  await expect(modal(page).getByRole('button', { name: 'Close' })).toBeDisabled();

  // Success: the dialog closes and the join lands exactly once — a room this
  // identity had never joined appears and opens (a real state transition,
  // not a re-open of something already there).
  await expect(modal(page)).toHaveCount(0, { timeout: 10_000 });
  await expect(page.getByRole('heading', { level: 1, name: 'Invited Workspace' })).toBeVisible();
  await expect(app.timeline.getByText('You joined as member', { exact: false })).toBeVisible();
  await app.showRoomsList();
  await expect(app.roomItem('Invited Workspace')).toHaveCount(1);
});

test('create: a keyboard double-submit fires exactly one request', async ({ app, page }) => {
  await app.gotoRoomsList({ mock_delay: 'room.create:800' });
  await page.locator('button.create-room:not(.join-room)').click();

  const name = modal(page).getByLabel('Room name');
  await name.fill('Double Submit Room');
  // Enter submits the form; the second Enter lands inside the busy window
  // (the mock holds room.create for 800ms) and must be swallowed.
  await name.press('Enter');
  await name.press('Enter');

  await expect(modal(page)).toHaveCount(0, { timeout: 10_000 });
  await expect(page.getByRole('heading', { level: 1, name: 'Double Submit Room' })).toBeVisible();
  // Wait for the full success transition (the session the create opened), then
  // count: a duplicated request would have produced a second same-named room.
  await app.showRoomsList();
  await expect(app.roomItem('Double Submit Room')).toContainText('Open');
  await expect(app.roomItem('Double Submit Room')).toHaveCount(1);
});

test('join: a keyboard double-submit consumes the single-use ticket once', async ({
  app,
  page,
}) => {
  await app.gotoRoomsList({ mock_ticket: TICKET_SUFFIX, mock_delay: 'room.join:800' });
  await page.getByRole('button', { name: 'Join with a ticket' }).click();

  await modal(page).getByLabel('Ticket').fill(`roomtkt1${TICKET_SUFFIX}`);
  // Enter in the peer-address input submits the form; tickets are single-use
  // in the mock, so a duplicated request would fail loudly with bad_ticket.
  const peerAddr = modal(page).getByLabel('Peer address', { exact: false });
  await peerAddr.press('Enter');
  await peerAddr.press('Enter');

  await expect(modal(page)).toHaveCount(0, { timeout: 10_000 });
  await app.showRoomsList();
  await expect(app.roomItem('Invited Workspace')).toHaveCount(1);
  // No stray failure from a second redemption anywhere.
  await expect(page.locator('.error-note')).toHaveCount(0);
});

test('join: failure restores interaction with a real error', async ({ app, page }) => {
  await app.gotoRoomsList();
  await page.getByRole('button', { name: 'Join with a ticket' }).click();

  await modal(page).getByLabel('Ticket').fill(`roomtkt1${'x'.repeat(90)}`);
  await modal(page).getByRole('button', { name: 'Join room' }).click();

  await expect(modal(page).locator('.error-note')).toBeVisible();
  await expect(modal(page).getByRole('button', { name: 'Join room' })).toBeEnabled();
  await page.keyboard.press('Escape');
  await expect(modal(page)).toHaveCount(0);
});

test('leave: initial focus is Cancel and immediate Enter cannot leave', async ({ app, page }) => {
  await app.gotoPopulated();
  await openLeaveDialog(app, page, MOCK_ROOMS.review);

  // Safe initial focus: Cancel, never the destructive submit.
  await expect(modal(page).getByRole('button', { name: 'Cancel' })).toBeFocused();
  await page.keyboard.press('Enter');

  // Enter activated Cancel: dialog closed, membership untouched.
  await expect(modal(page)).toHaveCount(0);
  await expect(app.rightPanel.getByRole('button', { name: 'Leave', exact: true })).toBeVisible();
  await returnToRoomsList(app);
  await expect(app.roomItem(MOCK_ROOMS.review)).not.toContainText('Left');
});

test('leave: a pending leave is contained, then applies once', async ({ app, page }) => {
  await app.gotoPopulated({ mock_delay: 'room.leave:1500' });
  await openLeaveDialog(app, page, MOCK_ROOMS.review);

  await modal(page).getByRole('button', { name: 'Leave room' }).click();
  await expect(modal(page).getByRole('button', { name: 'Leaving…' })).toBeDisabled();
  await expect(modal(page).getByRole('button', { name: 'Cancel' })).toBeDisabled();
  await page.keyboard.press('Escape');
  await expect(modal(page)).toBeVisible();
  await page.locator('.modal-backdrop').click({ position: { x: 5, y: 5 } });
  await expect(modal(page)).toBeVisible();
  await expect(modal(page).getByRole('button', { name: 'Close' })).toBeDisabled();

  // The departure lands exactly once: dialog closed, room marked Left, and the
  // route left the room it published a departure from — a room tool cannot
  // stay open over a room this identity is no longer in.
  await expect(modal(page)).toHaveCount(0, { timeout: 10_000 });
  await expect(page).toHaveURL(/\/rooms(\?|$)/);
  // A room you left is not lost — it moves into the collapsed "Left & removed"
  // disclosure (issue #64). Expand it to reach the row it now lives in.
  await app.showDeparted();
  await expect(app.roomItem(MOCK_ROOMS.review)).toBeDisabled();
  await expect(app.roomItem(MOCK_ROOMS.review)).toContainText('Left');
  await expect(app.sidebar).toBeVisible();
  if (!app.compact) {
    await expect(page.getByText('Choose a room.')).toBeVisible();
  }
});

test('leave: failure keeps the dialog actionable', async ({ app, page }) => {
  await app.gotoPopulated({ mock_fail: 'room.leave:1' });
  await openLeaveDialog(app, page, MOCK_ROOMS.review);

  await modal(page).getByRole('button', { name: 'Leave room' }).click();

  await expect(modal(page).locator('.error-note')).toBeVisible();
  await expect(modal(page).getByRole('button', { name: 'Leave room' })).toBeEnabled();
  await modal(page).getByRole('button', { name: 'Cancel' }).click();
  await expect(modal(page)).toHaveCount(0);
  // Still a member — the failure was real, nothing was applied.
  await returnToRoomsList(app);
  await expect(app.roomItem(MOCK_ROOMS.review)).not.toContainText('Left');
});
