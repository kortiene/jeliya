import { expect, test, MOCK_ROOMS } from './fixtures';
import { en } from '../src/l10n/en';

// Issue #53: a failed room.open must offer explicit Retry and Back to Rooms
// paths, retry the same room without selecting another first, and never
// render another room's data (or a comforting empty timeline) underneath.
//
// Every case here pins the failure to a deliberately selected room via
// `mock_fail=room.open:<fails>:1`: boot restores the last room and opens it
// (docs/room-workbench.md, decision 2), so the one allowed call is spent
// there and the next selection is what breaks. That is also why none of these
// tests branch on the shell any more — compact boots into the room too, so the
// failure arrives the same way on all three.

test('a failed room open offers Retry, and Retry restores the room', async ({ app, page }) => {
  await app.gotoPopulated({ mock_fail: 'room.open:1:1' });
  await app.showRoomsList();
  await app.roomItem(MOCK_ROOMS.design).click();

  // The route keeps naming the room whose open failed. A daemon error is a
  // state of that room, not a reason to bounce the user somewhere else
  // (decision 2: keep the route, surface the real error, offer Retry).
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/activity/);
  await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.design })).toBeVisible();

  // The error owns the pane: real failure, recovery actions, no fake timeline.
  const surface = page.locator('.room-error-surface');
  await expect(surface).toBeVisible();
  await expect(surface.getByText('Something went wrong')).toBeVisible();
  await expect(surface.getByText(en.commonTechnicalDetails, { exact: true })).toBeVisible();
  await expect(app.timeline).toHaveCount(0);
  await expect(app.composerTextarea).toHaveCount(0);
  await expect(page.getByText('No events yet')).toHaveCount(0);

  // Retry is a real, keyboard-operable control that re-opens THIS room.
  const retry = surface.getByRole('button', { name: 'Retry' });
  await retry.focus();
  await page.keyboard.press('Enter');

  await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.design })).toBeVisible();
  await expect(app.timeline).toBeVisible();
  await expect(app.timeline.getByText('Tokens v2 exploration lives here.')).toBeVisible();
  await expect(surface).toHaveCount(0);
  await expect(app.composerTextarea).toBeVisible();
});

test('re-selecting the errored room retries instead of doing nothing', async ({
  app,
  page,
  compact,
}) => {
  // Compact never puts the rooms list and the failed room's pane on screen
  // together, so re-selecting the selected room is not a gesture it has: the
  // only way back to the list is Back to Rooms, which moves the route off the
  // room and clears its session state — the next tap is then an ordinary open,
  // already covered above.
  test.skip(compact, 'the rooms list and the room pane are never co-visible on compact');
  await app.gotoPopulated({ mock_fail: 'room.open:2:1' });
  await app.roomItem(MOCK_ROOMS.design).click();

  const surface = page.locator('.room-error-surface');
  await expect(surface).toBeVisible();

  // Clicking the already-selected room must issue a real second attempt. The
  // route cannot change (it already names this room), so nothing about the
  // navigation can carry the intent — it consumes the second injected
  // failure…
  await app.roomItem(MOCK_ROOMS.design).click();
  await expect(surface).toBeVisible();

  // …so the next retry succeeds. Without the retry-on-reselect fix, this
  // click would hit the second failure and the test would fail.
  await surface.getByRole('button', { name: 'Retry' }).click();
  await expect(app.timeline).toBeVisible();
});

test('Back to Rooms escapes a room that cannot open', async ({ app, page, compact }) => {
  // The restored room opens fine; every later open fails — so the failure is
  // pinned to a deliberately selected, non-default room.
  await app.gotoPopulated({ mock_fail: 'room.open:9:1' });
  await app.showRoomsList();
  await app.roomItem(MOCK_ROOMS.design).click();

  const surface = page.locator('.room-error-surface');
  await expect(surface).toBeVisible();
  // Keyboard-operable, like Retry: focus + Enter, not a mouse click.
  const back = surface.getByRole('button', { name: 'Back to Rooms' });
  await back.focus();
  await page.keyboard.press('Enter');

  // The failed room is fully deselected — and because the route *is* the
  // selection, that is one fact rather than two that could disagree.
  await expect(page).toHaveURL(/\/rooms(\?|$)/);
  await expect(surface).toHaveCount(0);
  await expect(app.roomItem(MOCK_ROOMS.design)).toBeVisible();
  if (compact) {
    await expect(app.center).toBeHidden();
    await expect(app.sidebar).toBeVisible();
  } else {
    await expect(page.getByText('Choose a room.')).toBeVisible();
  }

  // The escape is durable (issue #88). Reloading holds `/rooms`, because
  // `/rooms` is an explicit destination and restoration only fills in a route
  // that named no room: nothing re-opens, so the failed room's error cannot
  // come back. The reload used to be expected to land in the healthy default
  // room instead — that is gone, and asserting it would now be asserting a
  // restore this route never asks for.
  await page.reload();
  await expect(page).toHaveURL(/\/rooms(\?|$)/);
  await expect(app.roomItem(MOCK_ROOMS.main)).toBeVisible();
  await expect(app.roomItem(MOCK_ROOMS.design)).toContainText('Closed');
  await expect(page.locator('.room-error-surface')).toHaveCount(0);

  // The other half of #88, which the route model does not fix by itself: the
  // last-room memory was cleared too, so even a load of `/` — which *does*
  // restore — comes back to the healthy default room and not to the one the
  // user just escaped.
  await app.gotoPopulated({ mock_fail: 'room.open:9:1' });
  await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.main })).toBeVisible();
  await expect(page.locator('.room-error-surface')).toHaveCount(0);
});

test("another room's data never renders under a failed open", async ({ app, page }) => {
  // The restored room opens and fills people/files/pipes; the next one fails.
  await app.gotoPopulated({ mock_fail: 'room.open:1:1' });
  await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.main })).toBeVisible();
  await expect(app.timeline.getByText('Kicked off the rooms protocol spec')).toBeVisible();

  // Positive control first — the main room's data really is on each room tool,
  // so the absence assertions below cannot pass vacuously.
  await app.goToRoomDest('Files');
  await expect(app.rightPanel.getByText('PRD_v0.2.pdf')).toBeVisible();
  await app.goToRoomDest('Pipes');
  await expect(app.rightPanel.getByText('127.0.0.1:3000')).toBeVisible();
  await app.goToRoomDest('People');
  await expect(app.rightPanel.getByText('Maya R.').first()).toBeVisible();

  // Leave by the room's own Back chain — tool → Activity → Rooms (decision 3).
  // On compact the inspector is the whole screen, so the rooms list is not
  // reachable from under it; above compact both steps are already true.
  await app.goToRoomDest('Activity');
  await app.showRoomsList();
  await app.roomItem(MOCK_ROOMS.design).click();
  await expect(page.locator('.room-error-surface')).toBeVisible();
  await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.design })).toBeVisible();

  // Walk every room tool under the failed room: nothing from the main room
  // bleeds through — each surface shows empty-room truth.
  await app.goToRoomDest('Files');
  await expect(app.roomTab('Files')).toHaveAttribute('aria-selected', 'true');
  await expect(app.rightPanel.getByText('PRD_v0.2.pdf')).toHaveCount(0);
  await app.goToRoomDest('Pipes');
  await expect(app.rightPanel.getByText('127.0.0.1:3000')).toHaveCount(0);
  await app.goToRoomDest('People');
  await expect(app.rightPanel.getByText('Maya R.')).toHaveCount(0);
  await expect(app.rightPanel.getByText('No members have synced', { exact: false })).toBeVisible();
});
