import { expect, test, MOCK_ROOMS } from './fixtures';

// Composing and sending messages through the mock daemon.
//
// Boot restores the last room, so `gotoPopulated()` already lands on a room's
// Activity — but *which* room follows from the mock's ordering. Both tests
// still open the room explicitly, so the composer under test is pinned to a
// known room's timeline rather than to whichever room boot happened to
// restore; reordering the fixture must not silently retarget these specs.

test('sends a message and renders it as a delivered event', async ({ app }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  const body = 'Hello from the regression harness';
  await app.composerTextarea.fill(body);
  await app.composerTextarea.press('Enter');

  // The message appears (optimistic pending first), then settles once the
  // mock daemon echoes the signed event — no "Sending…" residue remains.
  await expect(app.timeline.getByText(body)).toBeVisible();
  await expect(app.timeline.locator('.pending-line')).toHaveCount(0);
  await expect(app.composerTextarea).toHaveValue('');
});

test('Shift+Enter makes a new line instead of sending', async ({ app }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  await app.composerTextarea.fill('first line');
  await app.composerTextarea.press('Shift+Enter');
  await app.composerTextarea.pressSequentially('second line');

  await expect(app.composerTextarea).toHaveValue('first line\nsecond line');
  // Nothing was sent.
  await expect(app.timeline.getByText('first line')).toHaveCount(0);
});
