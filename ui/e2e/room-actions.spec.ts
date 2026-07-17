import { expect, test, MOCK_ROOMS } from './fixtures';

// Issue #54: Share File, Open Pipe, and Open in Pipes must land on a VISIBLE
// surface on compact viewports — the released bug updated the hidden
// right-panel tab and appeared to do nothing.

test('room-header Share File opens a visible Files surface', async ({ app, page, compact }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  await page.getByRole('button', { name: 'Share File' }).click();

  await expect(app.rightPanel).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Files', exact: false })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  await expect(app.rightPanel.getByRole('heading', { name: '5 shared files' })).toBeVisible();

  if (compact) {
    // Pane, panel tab, and bottom navigation may never contradict each other.
    await expect(app.center).toBeHidden();
    await expect(app.mobileTab('Files')).toHaveAttribute('aria-current', 'page');
    // Repeating the destination while already on it is idempotent.
    await app.mobileTab('Files').click();
    await expect(app.rightPanel).toBeVisible();
    await expect(app.mobileTab('Files')).toHaveAttribute('aria-current', 'page');
  }
});

test('room-header Open Pipe opens a visible Pipes surface', async ({ app, page, compact }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  await page.getByRole('button', { name: 'Open Pipe' }).click();

  await expect(app.rightPanel).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Pipes', exact: false })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  await expect(app.rightPanel.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();

  if (compact) {
    await expect(app.center).toBeHidden();
    await expect(app.mobileTab('Pipes')).toHaveAttribute('aria-current', 'page');
  }
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

  await expect(app.rightPanel).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Pipes', exact: false })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  if (compact) {
    await expect(app.center).toBeHidden();
    await expect(app.mobileTab('Pipes')).toHaveAttribute('aria-current', 'page');
  }

  // The referenced pipe row is identified: transiently marked, and durably
  // focused + on screen (focus outlives the 1.6s visual marker).
  await expect(app.rightPanel.locator('.pipe-row-flash')).toContainText('127.0.0.1:4000');
  const row = app.rightPanel.locator('.pipe-row', { hasText: '127.0.0.1:4000' });
  await expect(row).toBeFocused();
  await expect(row).toBeInViewport();
});

test('mobile: the panel tab strip keeps the bottom navigation truthful', async ({
  app,
  page,
  compact,
}) => {
  test.skip(!compact, 'the bottom bar only exists on compact');
  await app.gotoPopulated();
  await app.navigate('Files');
  await expect(app.mobileTab('Files')).toHaveAttribute('aria-current', 'page');

  // Switching to Pipes inside the panel's own tab strip must move the bottom
  // navigation with it — a Files highlight over Pipes content is a lie.
  await page.getByRole('tab', { name: 'Pipes', exact: false }).click();
  await expect(app.mobileTab('Pipes')).toHaveAttribute('aria-current', 'page');
  await expect(app.rightPanel.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();

  // Members is a sub-view of the current pane (it has no bottom-nav
  // destination): the pane and its highlight stay put.
  await page.getByRole('tab', { name: 'Members', exact: false }).click();
  await expect(app.mobileTab('Pipes')).toHaveAttribute('aria-current', 'page');
  await expect(app.rightPanel.getByRole('heading', { name: 'Room roster' })).toBeVisible();
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
