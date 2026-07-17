import { expect, test, MOCK_ROOMS } from './fixtures';

// Issue #52: a room must open at its newest timeline event, and a deliberate
// reading position must survive the pane being hidden and rooms being switched.
//
// Two mechanisms carry that, and both are load-bearing here. The compact shell
// hides panes with display:none instead of unmounting them
// (docs/room-workbench.md, decision 3): a hidden element measures zero and
// loses scrollTop, so the timeline has to reinstate its position the moment it
// is laid out again. Switching rooms genuinely unmounts it (the timeline is
// keyed by room), so there the position rides across on a saved view.

// The newest fixture event in the main room (mock.ts anchors it near "now").
const NEWEST_MAIN_EVENT = 'Sync convergence suite running (14/24 green).';

test('mobile: the room pane reveals at the newest event', async ({ app, compact }) => {
  test.skip(!compact, 'medium and wide keep the room pane laid out; this is the hidden-pane path');
  await app.gotoPopulated();

  // Boot restores the room and lands inside it (decision 2), so the auto-opened
  // room is on screen from its first paint and cannot mount hidden. The zeroed
  // measurements that mount had to survive are still reached, though — every
  // room tool display:none's the pane behind it — so this walks that cycle
  // instead.
  //
  // The baseline first: the restored room opens at its newest event. On very
  // short viewports the newest card can be taller than the visible timeline, so
  // the contract is: its row intersects the viewport and the scroller sits at
  // the bottom.
  await expect(app.timeline.getByText(NEWEST_MAIN_EVENT)).toBeVisible();
  await expect(app.timeline.locator('.timeline-row').last()).toBeInViewport();
  expect(await app.timelineBottomOffset()).toBeLessThanOrEqual(140);

  // A room tool owns the whole screen on compact, so the room pane goes
  // display:none — every measurement it had is now zero.
  await app.goToRoomDest('People');
  await expect(app.center).toBeHidden();

  // Revealed again, it must re-derive the bottom rather than sit at the
  // scrollTop that display:none reset for it.
  await app.goToRoomDest('Activity');
  await expect(app.timeline.getByText(NEWEST_MAIN_EVENT)).toBeVisible();
  await expect(app.timeline.locator('.timeline-row').last()).toBeInViewport();
  await expect.poll(() => app.timelineBottomOffset()).toBeLessThanOrEqual(140);
});

test('selecting a different room opens its latest activity', async ({ app }) => {
  // Boot restores the main room on every shell, so this is a switch between two
  // rooms and not a first open.
  await app.gotoPopulated();
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

  // The backlog has to be in before a reading position means anything: the
  // timeline renders (as skeletons) while room.open is still in flight, and a
  // scroll offset set against that empty scroller is silently dropped.
  await expect(app.timeline.getByText(NEWEST_MAIN_EVENT)).toBeVisible();

  // Scroll deliberately away from the bottom (display:none will wipe this).
  await app.timeline.evaluate((el) => {
    el.scrollTop = 300;
  });
  await expect.poll(() => app.timeline.evaluate((el) => el.scrollTop)).toBe(300);

  // A room tool hides the room pane without unmounting it, so the timeline
  // keeps its state and loses only its layout — the exact case decision 3
  // promises the reader will not notice.
  await app.goToRoomDest('People');
  await expect(app.center).toBeHidden();
  await app.goToRoomDest('Activity');
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

  // The room must really be open at its newest event before the reader scrolls
  // away from it. room.open is still in flight when the timeline first renders,
  // and scrolling an empty scroller sets nothing — the room would then be
  // remembered as stuck-to-bottom and this would assert against a position the
  // test never actually established.
  await expect(app.timeline.getByText(NEWEST_MAIN_EVENT)).toBeVisible();

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

test('stick-to-bottom keeps following new events', async ({ app, compact }) => {
  test.skip(compact, 'medium and wide invariant — must not regress while fixing mobile');
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
