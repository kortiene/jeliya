import { expect, test, MOCK_ROOMS } from './fixtures';
import type { AppDriver } from './fixtures';

// Issue #54: Share File, Open Pipe, and Open in Pipes must land on a VISIBLE
// surface — the released bug selected the hidden right-panel tab and appeared
// to do nothing. The mechanics now differ per shell (the tool takes the pane on
// compact, floats as a drawer on medium, opens a column on wide), but the
// guarantee does not: the surface an action names is the surface the user is
// looking at afterwards. Each action is one navigation, so the route it lands
// on is asserted alongside the surface — a route that says `files` while the
// user is looking at something else is the same lie in a new place.

/** Fire one of the room header's own actions, the way a user of that shell has
 *  to. On compact they live behind the app bar's ⋮ disclosure: an action row on
 *  the bar itself is what pushed the timeline under 180px at 320x568 (issue
 *  #61). On medium and wide they sit on the header directly. */
async function fireRoomAction(app: AppDriver, name: 'Share File' | 'Open Pipe') {
  if (app.compact) {
    await app.page.getByRole('button', { name: 'Room information' }).click();
  }
  await app.page.getByRole('button', { name }).click();
}

test('room-header Share File opens a visible Files surface', async ({ app, page, compact }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  await fireRoomAction(app, 'Share File');

  await expect(page).toHaveURL(/\/rooms\/[^/]+\/files(\?|$)/);
  await expect(app.rightPanel).toBeVisible();
  await expect(app.roomTab('Files')).toHaveAttribute('aria-selected', 'true');
  await expect(app.rightPanel.getByRole('heading', { name: '5 shared files' })).toBeVisible();

  if (compact) {
    // One pane at a time: the room gives way to the tool it opened, so there is
    // no second surface left for the action to have updated instead.
    await expect(app.center).toBeHidden();

    // Repeating the destination while already on it is idempotent — the route
    // is already `files`, so re-selecting it must neither toggle the surface
    // shut nor stack an entry the user has to press Back twice to undo.
    await app.roomTab('Files').click();
    await expect(page).toHaveURL(/\/rooms\/[^/]+\/files(\?|$)/);
    await expect(app.rightPanel).toBeVisible();
    await expect(app.roomTab('Files')).toHaveAttribute('aria-selected', 'true');
  }
});

test('room-header Open Pipe opens a visible Pipes surface', async ({ app, page, compact }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  await fireRoomAction(app, 'Open Pipe');

  await expect(page).toHaveURL(/\/rooms\/[^/]+\/pipes(\?|$)/);
  await expect(app.rightPanel).toBeVisible();
  await expect(app.roomTab('Pipes')).toHaveAttribute('aria-selected', 'true');
  await expect(app.rightPanel.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();

  if (compact) await expect(app.center).toBeHidden();
});

test('timeline Open in Pipes opens Pipes and identifies the pipe', async ({
  app,
  page,
  compact,
}) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  // The fixture's first pipe event is the logs pipe on 127.0.0.1:4000.
  await app.timeline.getByRole('button', { name: 'Open in Pipes' }).first().click();

  await expect(page).toHaveURL(/\/rooms\/[^/]+\/pipes(\?|$)/);
  await expect(app.rightPanel).toBeVisible();
  await expect(app.roomTab('Pipes')).toHaveAttribute('aria-selected', 'true');
  if (compact) await expect(app.center).toBeHidden();

  // The referenced pipe row is identified: transiently marked, and durably
  // focused + on screen (focus outlives the 1.6s visual marker).
  await expect(app.rightPanel.locator('.pipe-row-flash')).toContainText('127.0.0.1:4000');
  const row = app.rightPanel.locator('.pipe-row', { hasText: '127.0.0.1:4000' });
  await expect(row).toBeFocused();
  await expect(row).toBeInViewport();
});

test('Open in Pipes identifies the pipe even while pipe.list is still loading', async ({
  app,
}) => {
  // Hold every pipe.list response so the action fires into an empty list —
  // the focus request must survive until the list arrives, not be dropped.
  await app.gotoPopulated({ mock_delay: 'pipe.list:1200' });
  await app.openRoom(MOCK_ROOMS.main);
  await app.timeline.getByRole('button', { name: 'Open in Pipes' }).first().click();

  await expect(app.roomTab('Pipes')).toHaveAttribute('aria-selected', 'true');
  const row = app.rightPanel.locator('.pipe-row', { hasText: '127.0.0.1:4000' });
  await expect(row).toBeFocused({ timeout: 10_000 });
  await expect(row).toBeInViewport();
});

// Replaces 'mobile: the panel tab strip keeps the bottom navigation truthful'.
// That test pinned "the pane, the panel tab, and the bottom navigation may
// never contradict each other" — an invariant no implementation can break any
// more. Files and Pipes are room tools, so the bottom bar has no Files
// highlight left to hang over Pipes content, and the pane and the strip are
// both read off the one route (docs/room-workbench.md, decision 2) rather than
// tracked separately. The invariant that replaced it is between the three
// things that still exist: the route, the strip's selection, and the pane on
// screen. Those can only agree by construction if nothing re-derives them, so
// this pins that they do.
test('the room tab strip, the visible pane, and the route agree', async ({
  app,
  page,
  compact,
}) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/activity(\?|$)/);
  await expect(app.roomTab('Activity')).toHaveAttribute('aria-selected', 'true');

  await app.goToRoomDest('Pipes');
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/pipes(\?|$)/);
  await expect(app.roomTab('Pipes')).toHaveAttribute('aria-selected', 'true');
  await expect(app.roomTab('Activity')).toHaveAttribute('aria-selected', 'false');
  await expect(app.rightPanel.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();
  if (compact) await expect(app.center).toBeHidden();

  // Every tool moves all three together — People is not a sub-view of whatever
  // was open, it is a destination like the rest.
  await app.goToRoomDest('People');
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/people(\?|$)/);
  await expect(app.roomTab('People')).toHaveAttribute('aria-selected', 'true');
  await expect(app.roomTab('Pipes')).toHaveAttribute('aria-selected', 'false');
  await expect(app.rightPanel.getByRole('heading', { name: 'Room roster' })).toBeVisible();

  // Activity is a destination too, so closing the inspector is the same
  // navigation as any other — and it leaves the room's own surface on screen.
  await app.goToRoomDest('Activity');
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/activity(\?|$)/);
  await expect(app.roomTab('Activity')).toHaveAttribute('aria-selected', 'true');
  await expect(app.rightPanel).toBeHidden();
  await expect(app.center).toBeVisible();
  await expect(app.timeline).toBeVisible();
});

// The strip is the only pointer route between room tools, so opening one must
// not put it out of reach. Compact and wide each have an answer — the inspector
// carries the strip itself on compact, and on wide the tool opens beside it in
// a column of its own. Medium is the shell with neither: the drawer floats over
// the very workspace the strip lives in, and a tab that is visible, enabled,
// and announced as selected while a pointer lands on something else is the
// issue #54 lie told from the other side.
test('opening a room tool leaves the room tab strip usable', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);
  await app.goToRoomDest('Files');

  // Moving between tools is one click from wherever the user already is — not
  // "dismiss the tool you opened, then pick another off the strip underneath".
  await app.goToRoomDest('Pipes');
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/pipes(\?|$)/);
  await expect(app.roomTab('Pipes')).toHaveAttribute('aria-selected', 'true');
  await expect(app.rightPanel.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();
});

test('desktop: repeating Open in Pipes stays idempotent', async ({ app, compact }) => {
  test.skip(compact, 'on compact the source button lives in the hidden chat pane');
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  const openInPipes = app.timeline.getByRole('button', { name: 'Open in Pipes' }).first();
  await openInPipes.click();
  await expect(app.rightPanel.locator('.pipe-row-flash')).toContainText('127.0.0.1:4000');
  await openInPipes.click();
  // Same destination, same identified row — no toggling, no stacking.
  await expect(app.rightPanel.locator('.pipe-row-flash')).toContainText('127.0.0.1:4000');
  await expect(app.rightPanel.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();
});
