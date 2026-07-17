import { expect, test, MOCK_ROOMS } from './fixtures';

// Files and Pipes entry points. On desktop these are right-panel tabs; on
// compact they are standalone panes reached from the bottom tab bar.

test('opens the Files surface with the shared-file inventory', async ({ app, page, compact }) => {
  await app.gotoPopulated();
  await app.navigate('Files');

  await expect(app.rightPanel).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Files', exact: false })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  // The default room's five fixture files. Scoped to the panel: the same
  // file names also render inside timeline event cards.
  await expect(app.rightPanel.getByRole('heading', { name: '5 shared files' })).toBeVisible();
  await expect(app.rightPanel.getByText('PRD_v0.2.pdf')).toBeVisible();

  if (compact) {
    // Standalone pane: the room context label is the only room name on screen.
    await expect(page.locator('.panel-room-context')).toHaveText(MOCK_ROOMS.main);
    await expect(app.center).toBeHidden();
  }
});

test('opens the Pipes surface with live pipes and the expose form', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Pipes');

  await expect(app.rightPanel).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Pipes', exact: false })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  await expect(app.rightPanel.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();
  // The two fixture pipes for the default room (scoped: pipe targets also
  // render inside timeline event cards).
  await expect(app.rightPanel.getByText('127.0.0.1:3000')).toBeVisible();
  await expect(app.rightPanel.getByText('127.0.0.1:4000')).toBeVisible();
});

test('desktop right-panel tabs switch between members, files, and pipes', async ({
  app,
  page,
  compact,
}) => {
  test.skip(compact, 'the tab strip is a desktop-only affordance; compact uses the tab bar');
  await app.gotoPopulated();

  await expect(page.getByRole('heading', { name: 'Room roster' })).toBeVisible();
  await page.getByRole('tab', { name: 'Files', exact: false }).click();
  await expect(page.getByRole('heading', { name: '5 shared files' })).toBeVisible();
  await page.getByRole('tab', { name: 'Pipes', exact: false }).click();
  await expect(page.getByRole('heading', { name: 'Expose a pipe' })).toBeVisible();
  await page.getByRole('tab', { name: 'Members', exact: false }).click();
  await expect(page.getByRole('heading', { name: 'Room roster' })).toBeVisible();
});
