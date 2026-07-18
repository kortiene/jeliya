import { expect, test, MOCK_ROOMS } from './fixtures';

// The Files and Pipes room tools (docs/room-workbench.md, decision 1). Neither
// can answer anything without a room_id — the daemon requires one for
// file.list, file.share, pipe.list and pipe.expose — so both are reached from
// inside a room, through the room's tab strip, on every shell. What the shell
// changes is only where the chosen tool renders: a third column on wide, a
// drawer over the workspace on medium, the whole screen on compact.

test('opens the Files surface list-first, sharing behind a compact picker', async ({ app, page, shell }) => {
  // Boot restores the last room, so the strip is already on screen.
  await app.gotoPopulated();
  await app.goToRoomDest('Files');

  // The tool is in the URL, not in a second state machine beside it: the
  // surface survives a reload and a paste into a colleague's window.
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/files$/);
  await expect(app.rightPanel).toBeVisible();
  await expect(app.roomTab('Files')).toHaveAttribute('aria-selected', 'true');

  // List-first (#67): the shared-file list is what the panel leads with — the
  // default room's five fixture files render (scoped to the panel: the same
  // names also render inside timeline event cards), and the honest summary is
  // still here.
  await expect(app.rightPanel.getByRole('heading', { name: '5 shared files' })).toBeVisible();
  await expect(app.rightPanel.getByText('PRD_v0.2.pdf')).toBeVisible();

  // Sharing is a compact affordance, not a form standing open above the list:
  // the picker's own heading is absent until the button reveals it.
  await expect(app.rightPanel.getByRole('button', { name: 'Share a file' })).toBeVisible();
  await expect(app.rightPanel.getByRole('heading', { name: 'Choose a file to share' })).toHaveCount(0);
  await app.rightPanel.getByRole('button', { name: 'Share a file' }).click();
  await expect(app.rightPanel.getByRole('heading', { name: 'Choose a file to share' })).toBeVisible();

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

test('opens the Pipes surface action-first, live pipes intact', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.goToRoomDest('Pipes');

  await expect(page).toHaveURL(/\/rooms\/[^/]+\/pipes$/);
  await expect(app.rightPanel).toBeVisible();
  await expect(app.roomTab('Pipes')).toHaveAttribute('aria-selected', 'true');
  // Action-first (#67): the expose form is anchored at the top, reachable
  // without scrolling past the existing pipes — which remain visible below it.
  await expect(app.rightPanel.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();
  // The two fixture pipes for the default room (scoped: pipe targets also
  // render inside timeline event cards).
  await expect(app.rightPanel.getByText('127.0.0.1:3000')).toBeVisible();
  await expect(app.rightPanel.getByText('127.0.0.1:4000')).toBeVisible();
});

test('selecting a file deep-links to it and opens its inspector without losing the list', async ({
  app,
  page,
}) => {
  await app.gotoPopulated();
  await app.goToRoomDest('Files');

  const prd = app.rightPanel.locator('.file-row', { hasText: 'PRD_v0.2.pdf' });
  await prd.locator('.file-row-select').click();

  // The selection is a deep link — the file id is in the URL, so the inspector
  // survives a reload and a paste.
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/files\/.+/);
  await expect(prd).toHaveClass(/file-row-selected/);

  // The list is not lost: the other rows are still on screen, and the
  // selected file's own fetch control is now revealed in its inspector.
  await expect(app.rightPanel.getByText('room-protocol.md')).toBeVisible();
  await expect(prd.locator('.file-inspector')).toBeVisible();

  // Deselecting returns to the list URL (no item).
  await prd.locator('.file-row-select').click();
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/files$/);
  await expect(app.rightPanel.locator('.file-row-selected')).toHaveCount(0);
});

test('selecting a pipe deep-links to it, expose form still reachable', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.goToRoomDest('Pipes');

  const row = app.rightPanel.locator('.pipe-row', { hasText: '127.0.0.1:3000' });
  await row.locator('.pipe-row-head').click();

  await expect(page).toHaveURL(/\/rooms\/[^/]+\/pipes\/.+/);
  await expect(row).toHaveClass(/pipe-row-selected/);
  // The list and the action-first expose form both stay in place.
  await expect(app.rightPanel.getByText('127.0.0.1:4000')).toBeVisible();
  await expect(app.rightPanel.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();
});

test('timeline "Open in Files" deep-links to the file in the Files workspace', async ({
  app,
  page,
  shell,
}) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  await app.timeline.getByRole('button', { name: 'Open in Files' }).first().click();

  // Lands on the Files tool, deep-linked to the shared file (the item, not just
  // the dest), with the row selected.
  await expect(page).toHaveURL(/\/rooms\/[^/]+\/files\/.+/);
  await expect(app.roomTab('Files')).toHaveAttribute('aria-selected', 'true');
  await expect(app.rightPanel.locator('.file-row-selected')).toBeVisible();
  if (shell === 'compact') await expect(app.center).toBeHidden();
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
