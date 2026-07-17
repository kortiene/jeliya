import { expect, test, MOCK_ROOMS } from './fixtures';

// The Files and Pipes room tools (docs/room-workbench.md, decision 1). Neither
// can answer anything without a room_id — the daemon requires one for
// file.list, file.share, pipe.list and pipe.expose — so both are reached from
// inside a room, through the room's tab strip, on every shell. What the shell
// changes is only where the chosen tool renders: a third column on wide, a
// drawer over the workspace on medium, the whole screen on compact.

test('opens the Files surface with the shared-file inventory', async ({ app, page, shell }) => {
  // Boot restores the last room, so the strip is already on screen.
  await app.gotoPopulated();
  await app.goToRoomDest('Files');

  // The tool is in the URL, not in a second state machine beside it: the
  // surface survives a reload and a paste into a colleague's window.
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/files$/);
  await expect(app.rightPanel).toBeVisible();
  await expect(app.roomTab('Files')).toHaveAttribute('aria-selected', 'true');

  // The default room's five fixture files. Scoped to the panel: the same
  // file names also render inside timeline event cards.
  await expect(app.rightPanel.getByRole('heading', { name: '5 shared files' })).toBeVisible();
  await expect(app.rightPanel.getByText('PRD_v0.2.pdf')).toBeVisible();

  // Room context stays visible on every room-scoped surface, on every shell
  // (decision 3) — but each shell owes it from a different element.
  if (shell === 'compact') {
    // One pane at a time: the workspace and its header are gone, so the
    // panel's own label is the only room name left in the accessible tree.
    await expect(app.center).toBeHidden();
    await expect(page.locator('.panel-room-context')).toHaveText(MOCK_ROOMS.main);
  } else {
    // The workspace keeps its header behind the inspector, so the panel does
    // not repeat the name.
    await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.main })).toBeVisible();
  }
});

test('opens the Pipes surface with live pipes and the expose form', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.goToRoomDest('Pipes');

  await expect(page).toHaveURL(/\/rooms\/[^/]+\/pipes$/);
  await expect(app.rightPanel).toBeVisible();
  await expect(app.roomTab('Pipes')).toHaveAttribute('aria-selected', 'true');
  await expect(app.rightPanel.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();
  // The two fixture pipes for the default room (scoped: pipe targets also
  // render inside timeline event cards).
  await expect(app.rightPanel.getByText('127.0.0.1:3000')).toBeVisible();
  await expect(app.rightPanel.getByText('127.0.0.1:4000')).toBeVisible();
});

test('the room tab strip moves between every room tool', async ({ app, page }) => {
  // This no longer skips the phone viewports. The strip used to be a
  // desktop-only affordance that compact replaced with bottom-bar tabs; it is
  // now the room's one navigation at every width, which is what lets a room
  // destination mean the same thing on all three shells (decision 3). On
  // compact the inspector renders the same strip itself.
  await app.gotoPopulated();

  // Activity is a destination like the others — the room with no tool open —
  // rather than a synonym for "a room is selected".
  await expect(app.roomTab('Activity')).toHaveAttribute('aria-selected', 'true');
  await expect(app.rightPanel).toBeHidden();

  await app.goToRoomDest('People');
  await expect(app.rightPanel.getByRole('heading', { name: 'Room roster' })).toBeVisible();

  // The fixture room's four agent members. "Agents & Runs" is this room's
  // agents; the global Agent Fleet answers a different question and is a
  // different destination (decision 1).
  await app.goToRoomDest('Agents & Runs');
  await expect(app.rightPanel.locator('.agent-card')).toHaveCount(4);

  await app.goToRoomDest('Files');
  await expect(app.rightPanel.getByRole('heading', { name: '5 shared files' })).toBeVisible();

  await app.goToRoomDest('Pipes');
  await expect(app.rightPanel.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();

  // Selecting Activity closes the inspector: collapsing it IS navigating, so
  // "which tool" and "is it open" have nothing left to disagree about.
  await app.goToRoomDest('Activity');
  await expect(app.rightPanel).toBeHidden();
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/activity$/);
});

test('Files and Pipes are room tools, not global destinations', async ({ app, page, shell }) => {
  // Both surfaces used to be reachable from global navigation, which made a
  // room-scoped tool look like a place you could stand without a room — the
  // destination was always secretly about one room, chosen elsewhere. The
  // reachability this file asserts above is only honest if the global bar has
  // stopped offering the same tools roomlessly, so pin the absence.
  await app.gotoRoomsList();

  const primary =
    shell === 'compact'
      ? app.tabBar
      : page.getByRole('navigation', { name: 'Primary', exact: true });

  await expect(primary.getByRole('button')).toHaveCount(3);
  await expect(primary.getByRole('button', { name: 'Files' })).toHaveCount(0);
  await expect(primary.getByRole('button', { name: 'Pipes' })).toHaveCount(0);
});
