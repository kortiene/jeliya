import { expect, test, MOCK_ROOMS } from './fixtures';

// The compact room app bar (issue #61; docs/room-workbench.md, decision 3).
//
// The budget at 320x568 is the point of this file: the app bar may not exceed
// 150px, and the timeline must keep at least 180px above the composer. The old
// header spent that budget on a wrapping action row and a peer-chip strip that
// grew with the room, so the height depended on how many peers happened to be
// connected — a layout you cannot make a promise about.

test.describe('the 320x568 budget', () => {
  test.skip(({ viewport }) => (viewport?.width ?? 0) !== 320, 'the budget is a 320px promise');

  test('the app bar stays under 150px and leaves the timeline 180px', async ({ app, page }) => {
    await app.gotoPopulated();
    await app.openRoom(MOCK_ROOMS.main);

    const bar = await page.locator('.room-appbar').evaluate((el) => el.getBoundingClientRect().height);
    expect(bar).toBeLessThanOrEqual(150);

    const timeline = await app.timeline.evaluate((el) => el.getBoundingClientRect().height);
    expect(timeline).toBeGreaterThanOrEqual(180);
  });

  test('the room with the most peers does not cost the timeline its height', async ({ app, page }) => {
    // The height must be a constant, not a function of room contents.
    await app.gotoPopulated();
    await app.openRoom(MOCK_ROOMS.main);
    const withPeers = await page.locator('.room-appbar').evaluate((el) => el.getBoundingClientRect().height);

    await app.openRoom(MOCK_ROOMS.design);
    const other = await page.locator('.room-appbar').evaluate((el) => el.getBoundingClientRect().height);
    expect(other).toBe(withPeers);
  });

  test('no action or status chip overflows horizontally', async ({ app, page }) => {
    await app.gotoPopulated();
    await app.openRoom(MOCK_ROOMS.main);
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1),
    ).toBe(false);

    // Including once the room-information disclosure is open, which is where
    // the peer paths and the room's own actions now live.
    await page.getByRole('button', { name: 'Room information' }).click();
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1),
    ).toBe(false);
  });
});

test.describe('compact room navigation', () => {
  test.skip(({ compact }) => !compact, 'the compact shell only');

  test('title, connectivity, Invite, Back, and the disclosure are all reachable', async ({ app, page }) => {
    await app.gotoPopulated();
    await app.openRoom(MOCK_ROOMS.main);

    await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.main })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Back to Rooms' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Invite' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Room information' })).toBeVisible();
    await expect(page.locator('.appbar-sub')).toContainText(/member|Loading/);
  });

  test('peer-path detail lives behind the room-information disclosure', async ({ app, page }) => {
    await app.gotoPopulated();
    await app.openRoom(MOCK_ROOMS.main);

    // Diagnostic detail is reachable, but it does not compete with the app bar
    // for a 320px phone's vertical budget.
    await expect(page.locator('.peer-strip')).toBeHidden();
    await page.getByRole('button', { name: 'Room information' }).click();
    await expect(page.locator('.appbar-info')).toBeVisible();
    await expect(page.locator('.room-info-facts')).toContainText('Session');
  });

  test('the bottom bar gives way to the room, and Back brings it back', async ({ app, page }) => {
    await app.gotoRoomsList();
    await expect(app.tabBar).toBeVisible();

    await app.roomItem(MOCK_ROOMS.main).click();
    await expect(app.timeline).toBeVisible();
    // Inside a room the global bar is gone — the room's app bar is the chrome.
    await expect(app.tabBar).toBeHidden();

    await page.getByRole('button', { name: 'Back to Rooms' }).click();
    await expect(app.tabBar).toBeVisible();
    await expect(app.roomItem(MOCK_ROOMS.main)).toBeVisible();
  });

  test('the bottom bar carries only the global destinations', async ({ app }) => {
    await app.gotoRoomsList();
    // Files and Pipes are room tools: a bottom-bar tab for them would say you
    // can stand there without a room, and you cannot.
    await expect(app.tabBar.getByRole('button')).toHaveCount(3);
    await expect(app.mobileTab('Rooms')).toBeVisible();
    await expect(app.mobileTab('Agent Fleet')).toBeVisible();
    await expect(app.mobileTab('Settings')).toBeVisible();
  });

  test('Back walks room tool -> Activity -> Rooms before leaving Jeliya', async ({ app, page }) => {
    await app.gotoRoomsList();
    await app.roomItem(MOCK_ROOMS.main).click();
    await expect(page).toHaveURL(/\/rooms\/[^/]+\/activity$/);

    await app.goToRoomDest('Files');
    await expect(page).toHaveURL(/\/rooms\/[^/]+\/files$/);

    await page.goBack();
    await expect(page).toHaveURL(/\/rooms\/[^/]+\/activity$/);
    await expect(app.timeline).toBeVisible();

    await page.goBack();
    await expect(page).toHaveURL(/\/rooms$/);
    await expect(app.roomItem(MOCK_ROOMS.main)).toBeVisible();
  });

  test('room context stays visible on every room destination', async ({ app, page }) => {
    await app.gotoPopulated();
    await app.openRoom(MOCK_ROOMS.main);

    // A room tool takes the whole screen here, so it must say which room it is
    // about — the room header is off in the hidden workspace pane.
    await app.goToRoomDest('Files');
    await expect(page.locator('.panel-room-context')).toContainText(MOCK_ROOMS.main);
    await app.goToRoomDest('Pipes');
    await expect(page.locator('.panel-room-context')).toContainText(MOCK_ROOMS.main);
  });
});
