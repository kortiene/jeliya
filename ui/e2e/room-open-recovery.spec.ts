import { expect, test, MOCK_ROOMS } from './fixtures';

// Issue #53: a failed room.open must offer explicit Retry and Back to Rooms
// paths, retry the same room without selecting another first, and never
// render another room's data (or a comforting empty timeline) underneath.

test('a failed room open offers Retry, and Retry restores the room', async ({
  app,
  page,
  compact,
}) => {
  // Desktop auto-opens the room (one visible failure); on compact the reveal
  // tap itself is the instinctive first retry, so spend two failures there.
  await app.gotoPopulated({ mock_fail: compact ? 'room.open:2' : 'room.open:1' });
  const surface = page.locator('.room-error-surface');
  if (compact) {
    // Wait for the hidden auto-open to fail before tapping, so the tap is
    // deterministically the second (and last) injected failure.
    await expect(surface).toHaveCount(1);
    await app.roomItem(MOCK_ROOMS.main).click();
  }

  // The error owns the pane: real failure, recovery actions, no fake timeline.
  await expect(surface).toBeVisible();
  await expect(surface.getByText('Something went wrong')).toBeVisible();
  await expect(surface.getByText('Technical details')).toBeVisible();
  await expect(app.timeline).toHaveCount(0);
  await expect(app.composerTextarea).toHaveCount(0);
  await expect(page.getByText('No events yet')).toHaveCount(0);

  // Retry is a real, keyboard-operable control that re-opens THIS room.
  const retry = surface.getByRole('button', { name: 'Retry' });
  await retry.focus();
  await page.keyboard.press('Enter');

  await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.main })).toBeVisible();
  await expect(app.timeline).toBeVisible();
  await expect(app.timeline.getByText('Kicked off the rooms protocol spec')).toBeVisible();
  await expect(surface).toHaveCount(0);
  await expect(app.composerTextarea).toBeVisible();
});

test('re-selecting the errored room retries instead of doing nothing', async ({
  app,
  page,
  compact,
}) => {
  test.skip(compact, 'covered by the reveal-tap in the Retry test; the sidebar is a desktop rail here');
  await app.gotoPopulated({ mock_fail: 'room.open:2' });

  const surface = page.locator('.room-error-surface');
  await expect(surface).toBeVisible();

  // Clicking the already-selected room must issue a real second attempt —
  // it consumes the second injected failure…
  await app.roomItem(MOCK_ROOMS.main).click();
  await expect(surface).toBeVisible();

  // …so the next retry succeeds. Without the retry-on-reselect fix, this
  // click would hit the second failure and the test would fail.
  await surface.getByRole('button', { name: 'Retry' }).click();
  await expect(app.timeline).toBeVisible();
});

test('Back to Rooms escapes a room that cannot open', async ({ app, page, compact }) => {
  // The default room opens fine; every later open fails — so the failure is
  // pinned to a deliberately selected, non-default room.
  await app.gotoPopulated({ mock_fail: 'room.open:9:1' });
  if (compact) await app.mobileTab('Rooms').click();
  await app.roomItem(MOCK_ROOMS.design).click();

  const surface = page.locator('.room-error-surface');
  await expect(surface).toBeVisible();
  // Keyboard-operable, like Retry: focus + Enter, not a mouse click.
  const back = surface.getByRole('button', { name: 'Back to Rooms' });
  await back.focus();
  await page.keyboard.press('Enter');

  // The failed room is fully deselected — nothing renders under it.
  await expect(surface).toHaveCount(0);
  await expect(app.roomItem(MOCK_ROOMS.design)).toBeVisible();
  if (!compact) {
    await expect(page.getByText('Select a room')).toBeVisible();
  } else {
    await expect(app.center).toBeHidden();
    await expect(app.sidebar).toBeVisible();
  }

  // The escape is durable: a reload must not auto-open straight back into
  // the failed room (its last-room memory is cleared) — the app boots into
  // the healthy default room with no error surface.
  await page.reload();
  await expect(app.roomItem(MOCK_ROOMS.main)).toBeVisible();
  await expect(app.roomItem(MOCK_ROOMS.main)).toContainText('Active', { timeout: 10_000 });
  await expect(page.locator('.room-error-surface')).toHaveCount(0);
});

test('another room\'s data never renders under a failed open', async ({ app, page }) => {
  // First open succeeds and fills members/files/pipes; the next one fails.
  await app.gotoPopulated({ mock_fail: 'room.open:1:1' });
  await app.openRoom(MOCK_ROOMS.main);
  await expect(app.timeline.getByText('Kicked off the rooms protocol spec')).toBeVisible();

  // Positive control first — the main room's data really is on each panel
  // tab, so the absence assertions below cannot pass vacuously.
  await app.navigate('Files');
  await expect(app.rightPanel.getByText('PRD_v0.2.pdf')).toBeVisible();
  await page.getByRole('tab', { name: 'Pipes', exact: false }).click();
  await expect(app.rightPanel.getByText('127.0.0.1:3000')).toBeVisible();
  await page.getByRole('tab', { name: 'Members', exact: false }).click();
  await expect(app.rightPanel.getByText('Maya R.').first()).toBeVisible();

  if (app.compact) await app.mobileTab('Rooms').click();
  await app.roomItem(MOCK_ROOMS.design).click();
  await expect(page.locator('.room-error-surface')).toBeVisible();
  await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.design })).toBeVisible();

  // Walk every panel tab under the failed room: nothing from the main room
  // bleeds through — each surface shows empty-room truth.
  await app.navigate('Files');
  await expect(page.getByRole('tab', { name: 'Files', exact: false })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  await expect(app.rightPanel.getByText('PRD_v0.2.pdf')).toHaveCount(0);
  await page.getByRole('tab', { name: 'Pipes', exact: false }).click();
  await expect(app.rightPanel.getByText('127.0.0.1:3000')).toHaveCount(0);
  await page.getByRole('tab', { name: 'Members', exact: false }).click();
  await expect(app.rightPanel.getByText('Maya R.')).toHaveCount(0);
  await expect(app.rightPanel.getByText('No members have synced', { exact: false })).toBeVisible();
});
