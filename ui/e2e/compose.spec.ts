import { expect, test, MOCK_ROOMS } from './fixtures';

// Composing and sending messages through the mock daemon.
//
// Boot restores the last room, so `gotoPopulated()` already lands on a room's
// Activity — but *which* room follows from the mock's ordering. Every test still
// opens the room explicitly, so the composer under test is pinned to a known
// room's timeline rather than to whichever room boot happened to restore;
// reordering the fixture must not silently retarget these specs.
//
// Enter behavior forks by shell (#67 P20): desktop sends on Enter (Shift+Enter
// is a newline); on compact Enter inserts a newline and the ➤ button is the
// explicit send — so the "Enter to send" hint, false there, is withheld. That
// mirrors the Flutter composer (hardware Enter via a key handler, soft-keyboard
// newline, width-gated hint).

test('desktop: Enter sends and renders it as a delivered event', async ({ app, compact }) => {
  test.skip(compact, 'compact inserts a newline on Enter — see the mobile test');
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
  // Desktop keeps the truthful "Enter to send" hint.
  await expect(app.page.getByText('Enter to send', { exact: false })).toBeVisible();
});

test('desktop: Shift+Enter makes a new line instead of sending', async ({ app, compact }) => {
  test.skip(compact, 'compact inserts a newline on Enter — see the mobile test');
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  await app.composerTextarea.fill('first line');
  await app.composerTextarea.press('Shift+Enter');
  await app.composerTextarea.pressSequentially('second line');

  await expect(app.composerTextarea).toHaveValue('first line\nsecond line');
  // Nothing was sent.
  await expect(app.timeline.getByText('first line')).toHaveCount(0);
});

test('mobile: Enter inserts a newline and Send is the explicit control', async ({ app, compact }) => {
  test.skip(!compact, 'desktop sends on Enter — see the desktop tests');
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  await app.composerTextarea.fill('first line');
  await app.composerTextarea.press('Enter');
  await app.composerTextarea.pressSequentially('second line');

  // Enter inserted a newline; nothing was sent.
  await expect(app.composerTextarea).toHaveValue('first line\nsecond line');
  await expect(app.timeline.getByText('first line')).toHaveCount(0);

  // The "Enter to send" claim is false here, so the hint is never shown.
  await expect(app.page.getByText('Enter to send', { exact: false })).toHaveCount(0);

  // Send is the explicit control, and it works.
  await app.page.getByRole('button', { name: 'Send message' }).click();
  await expect(app.composerTextarea).toHaveValue('');
  await expect(app.timeline.locator('.pending-line')).toHaveCount(0);
});

test('the composer attachment shares a file', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  // The touch-native attachment (#67 P20) shares through the same verified flow
  // as paste/drop — the mock emits a file_shared event, so it lands in the
  // timeline. setInputFiles drives the (display:none) input's change directly,
  // exactly as the attach button does.
  await page.locator('.composer input[type="file"]').setInputFiles({
    name: 'harness-note.txt',
    mimeType: 'text/plain',
    buffer: Buffer.from('shared via the touch attachment'),
  });

  await expect(app.timeline.getByText('harness-note.txt')).toBeVisible();
});

test('a failed share never blocks sending a message', async ({ app, page, compact }) => {
  // Send and share are independent flags (#67 P20): a failed — or in-flight —
  // share must never disable Send, nor the reverse.
  await app.gotoPopulated({ mock_fail: 'file.share:1' });
  await app.openRoom(MOCK_ROOMS.main);

  await page.locator('.composer input[type="file"]').setInputFiles({
    name: 'blocked.txt',
    mimeType: 'text/plain',
    buffer: Buffer.from('this share is rigged to fail'),
  });
  // The share surfaces its error inline...
  await expect(page.locator('.composer .error-note')).toBeVisible();

  // ...and the message still sends, on either shell's send affordance.
  const body = 'send is not blocked by a failed share';
  await app.composerTextarea.fill(body);
  if (compact) await page.getByRole('button', { name: 'Send message' }).click();
  else await app.composerTextarea.press('Enter');
  await expect(app.timeline.getByText(body)).toBeVisible();
});
