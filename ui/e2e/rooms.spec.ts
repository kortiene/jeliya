import { expect, test, MOCK_ROOMS } from './fixtures';

// Room list + selecting/opening rooms, on the populated fixture.

test('lists every fixture room with membership metadata', async ({ app }) => {
  await app.gotoPopulated();
  for (const name of Object.values(MOCK_ROOMS)) {
    await expect(app.roomItem(name)).toBeVisible();
  }
  await expect(app.roomItem(MOCK_ROOMS.main)).toContainText('members');
});

test('opens the default room into a populated timeline', async ({ app, page, compact }) => {
  await app.gotoPopulated();

  if (compact) {
    // Compact lands on the Rooms pane; the chat pane must stay hidden until
    // the user picks a room.
    await expect(app.center).toBeHidden();
    await app.openRoom(MOCK_ROOMS.main);
  } else {
    // Desktop auto-opens the default room in the center column.
    await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.main })).toBeVisible();
  }

  await expect(
    app.timeline.getByText('Kicked off the rooms protocol spec', { exact: false }),
  ).toBeVisible();
  await expect(app.timeline.getByText('created the room', { exact: false })).toBeVisible();
});

test('switches rooms and shows the selected room content', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.design);
  await expect(app.timeline.getByText('Tokens v2 exploration lives here.')).toBeVisible();

  await app.openRoom(MOCK_ROOMS.review);
  await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.review })).toBeVisible();
  await expect(
    app.timeline.getByText('Weekly product review — drop artifacts before Friday.'),
  ).toBeVisible();
});
