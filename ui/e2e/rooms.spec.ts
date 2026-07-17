import { expect, test, MOCK_ROOMS } from './fixtures';

// Room list + selecting/opening rooms, on the populated fixture.

test('lists every fixture room with membership metadata', async ({ app, page }) => {
  await app.gotoRoomsList();

  for (const name of Object.values(MOCK_ROOMS)) {
    // A row states two facts and names each one exactly (docs/room-workbench.md,
    // decision 4): how many members the room has, and whether this daemon holds
    // a session for it.
    await expect(app.roomItem(name)).toContainText(/\d+ members? · (Open|Closed)/);
  }

  // A room this daemon has never opened reads Closed — the session word, which
  // is a different fact from the membership count beside it.
  await expect(app.roomItem(MOCK_ROOMS.design)).toContainText(/\d+ members · Closed/);

  // "Active" is retired as a display label. On this row it meant a live local
  // session while meaning signed membership on the wire, and one word cannot
  // honestly be both.
  await expect(page.getByRole('navigation', { name: 'Rooms' })).not.toContainText('Active');
});

test('restores the default room into a populated timeline', async ({ app, page }) => {
  await app.gotoPopulated();

  // Loading `/` restores the last room, so the app lands *inside* it on every
  // shell — on compact that is the room pane, not the rooms list.
  await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.main })).toBeVisible();
  await expect(app.timeline.getByText('Kicked off the rooms protocol spec', { exact: false })).toBeVisible();
  await expect(app.timeline.getByText('created the room', { exact: false })).toBeVisible();
});

// Boot used to leave compact on the rooms list, and the guard that mattered was
// "the chat pane stays hidden until the user picks a room". It cannot be
// violated any more — boot lands in the room by design. What replaces it is the
// obligation the restore now carries: `/` is *replaced* with /rooms and the room
// is *pushed* on top, so the first Back press leaves the room the user never
// chose to stand in, rather than leaving Jeliya (decision 3 — Back is truthful:
// room destination → Activity → Rooms → out).
test('back from the restored room lands on the rooms list', async ({ app, page }) => {
  await app.gotoPopulated();

  await page.goBack();

  await expect(page).toHaveURL(/\/rooms$/);
  await expect(app.roomItem(MOCK_ROOMS.main)).toBeVisible();
  // And it stays left: nothing may re-restore the room behind an explicit
  // escape (issue #88).
  await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.main })).toBeHidden();
});

test('switches rooms and shows the selected room content', async ({ app, page }) => {
  await app.gotoPopulated();

  await app.openRoom(MOCK_ROOMS.design);
  await expect(app.timeline.getByText('Tokens v2 exploration lives here.')).toBeVisible();

  await app.openRoom(MOCK_ROOMS.review);
  await expect(page.getByRole('heading', { level: 1, name: MOCK_ROOMS.review })).toBeVisible();
  await expect(app.timeline.getByText('Weekly product review — drop artifacts before Friday.')).toBeVisible();
  // The outgoing room's events may not linger under the incoming room's title:
  // a message rendered under the wrong room name misstates who can read it.
  await expect(app.timeline.getByText('Tokens v2 exploration lives here.')).toBeHidden();
});
