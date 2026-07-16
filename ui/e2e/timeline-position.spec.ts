import { expect, test, MOCK_ROOMS } from './fixtures';

// Issue #52: mobile rooms must open at the newest timeline event even though
// the room is auto-opened while its pane is display:none, and a deliberate
// reading position must survive pane hiding and room switches.

// The newest fixture event in the main room (mock.ts anchors it near "now").
const NEWEST_MAIN_EVENT = 'Sync convergence suite running (14/24 green).';

test('mobile: the auto-opened default room reveals at the newest event', async ({
  app,
  compact,
}) => {
  test.skip(!compact, 'desktop mounts the timeline visible; this is the hidden-mount path');
  await app.gotoPopulated();

  // The room was opened while the chat pane was hidden; first reveal must
  // land within 140px of the bottom with the newest event exposed. On very
  // short viewports the newest card can be taller than the visible timeline,
  // so the contract is: its row intersects the viewport and the scroller sits
  // at the bottom.
  await app.openRoom(MOCK_ROOMS.main);
  await expect(app.timeline.getByText(NEWEST_MAIN_EVENT)).toBeVisible();
  await expect(app.timeline.locator('.timeline-row').last()).toBeInViewport();
  expect(await app.timelineBottomOffset()).toBeLessThanOrEqual(140);
});

test('selecting a different room opens its latest activity', async ({ app }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);
  await app.openRoom(MOCK_ROOMS.design);

  await expect(app.timeline.getByText('Tokens v2 exploration lives here.')).toBeVisible();
  await expect(app.timeline.locator('.timeline-row').last()).toBeInViewport();
  expect(await app.timelineBottomOffset()).toBeLessThanOrEqual(140);
});

test('mobile: hiding and revealing the room pane keeps the reading position', async ({
  app,
  compact,
}) => {
  test.skip(!compact, 'panes are only hidden on compact');
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  // Scroll deliberately away from the bottom (display:none will wipe this).
  await app.timeline.evaluate((el) => {
    el.scrollTop = 300;
  });
  await expect.poll(() => app.timeline.evaluate((el) => el.scrollTop)).toBe(300);

  await app.mobileTab('Rooms').click();
  await expect(app.center).toBeHidden();
  await app.roomItem(MOCK_ROOMS.main).click();
  await expect(app.timeline).toBeVisible();

  // Reinstated, not reset to the top and not jumped to the bottom.
  await expect
    .poll(() => app.timeline.evaluate((el) => Math.abs(el.scrollTop - 300)))
    .toBeLessThan(2);
});

test('returning to a scrolled-up room preserves the position and offers new activity', async ({
  app,
}) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  // Deliberately read from the top…
  await app.timeline.evaluate((el) => {
    el.scrollTop = 0;
  });
  await expect.poll(() => app.timeline.evaluate((el) => el.scrollTop)).toBe(0);

  // …leave for another room, then come back.
  await app.openRoom(MOCK_ROOMS.design);
  await app.openRoom(MOCK_ROOMS.main);

  // Wait for the reloaded backlog (the oldest event is on screen only if the
  // reading position was really preserved), then confirm: top, not bottom.
  await expect(app.timeline.getByText('created the room', { exact: false })).toBeInViewport();
  await expect.poll(() => app.timeline.evaluate((el) => el.scrollTop)).toBe(0);
  expect(await app.timelineBottomOffset()).toBeGreaterThan(140);

  // New activity surfaces as a control, not a jump.
  await app.composerTextarea.fill('note to self while reading history');
  await app.composerTextarea.press('Enter');
  const newMessages = app.page.locator('.new-messages');
  await expect(newMessages).toBeVisible();
  await expect.poll(() => app.timeline.evaluate((el) => el.scrollTop)).toBe(0);

  // The control takes the reader to the newest event on demand.
  await newMessages.click();
  await expect(newMessages).toBeHidden();
  await expect.poll(() => app.timelineBottomOffset()).toBeLessThanOrEqual(140);
});

test('desktop: stick-to-bottom keeps following new events', async ({ app, compact }) => {
  test.skip(compact, 'desktop invariant — must not regress while fixing mobile');
  await app.gotoPopulated();
  await expect(app.timeline.getByText(NEWEST_MAIN_EVENT)).toBeInViewport();

  // Own sends stay pinned to the bottom…
  await app.composerTextarea.fill('following the live conversation');
  await app.composerTextarea.press('Enter');
  await expect(app.timeline.getByText('following the live conversation')).toBeInViewport();
  expect(await app.timelineBottomOffset()).toBeLessThanOrEqual(140);

  // …and so does the next simulated live event from the mock daemon.
  await expect(app.timeline.getByText('Integration tests passing (17/24)')).toBeInViewport({
    timeout: 15_000,
  });
  expect(await app.timelineBottomOffset()).toBeLessThanOrEqual(140);
});
