import { expect, test } from '@playwright/test';

// One deterministic scenario covering exactly the five steps issue #158 asks a
// Dioxus/WASM slice to prove against a real supervised `jeliyad` on fresh state:
// bootstrap, room list, room open, message send, and one live `room.event` push.
//
// Web-first assertions only, no arbitrary sleeps — the same rule the React
// suite follows (CONTRIBUTING.md).
test('the Dioxus slice boots, opens a room, sends, and renders the live push', async ({ page }) => {
  const consoleErrors: string[] = [];
  page.on('console', (m) => {
    if (m.type() === 'error') consoleErrors.push(m.text());
  });
  page.on('pageerror', (e) => consoleErrors.push(String(e)));

  await page.goto('/');

  // 1. Bootstrap. A brand-new data dir has no identity and no rooms; the slice
  //    creates both and reaches Ready, or it renders why it could not.
  const boot = page.locator('#spike-error');
  const app = page.locator('#spike-app');
  await expect(app.or(boot)).toBeVisible({ timeout: 30_000 });
  if (await boot.isVisible()) {
    throw new Error(`slice failed to bootstrap: ${await boot.textContent()}`);
  }

  // The protocol major is read once after connecting, before trusting anything.
  await expect(page.locator('#spike-status')).toContainText('protocol 1');

  // 2. Room list, rendered from a real `room.list`.
  const room = page.locator('.room-select').first();
  await expect(room).toBeVisible();
  await expect(page.locator('.room-name').first()).toHaveText('Spike room');

  // 3. Room open. `room.open` is what starts this room's pushes.
  await room.click();
  const timeline = page.locator('#spike-timeline');
  await expect(timeline).toBeVisible();

  // The composer only exists once a room is open.
  const composer = page.locator('#composer-input');
  await expect(composer).toBeVisible();

  // 4. Send. The slice renders no optimistic echo, so anything that appears
  //    below came back from the daemon.
  //
  //    Measured against the state this test found, not against an assumed
  //    empty room: the fixture gives each `playwright test` invocation one
  //    daemon, so a second spec in the same run shares this room.
  const bubbles = timeline.locator('.msg-bubble');
  const before = await bubbles.count();
  const pushesBefore = Number(await timeline.getAttribute('data-pushes'));

  const body = `hello from the dioxus spike ${Date.now()}`;
  await composer.fill(body);
  await page.locator('#spike-send').click();

  // 5. The live `room.event` push. This assertion is the whole spike: the
  //    message is on screen only because the daemon pushed it back, and
  //    exactly one new event arrived for it.
  await expect(bubbles.filter({ hasText: body })).toHaveCount(1);
  await expect(bubbles).toHaveCount(before + 1);
  await expect(timeline).toHaveAttribute('data-pushes', String(pushesBefore + 1));

  // The composer clears only after the daemon accepted the send.
  await expect(composer).toHaveValue('');

  // The stylesheet is `ui/src/styles.css`, copied verbatim. If the design
  // system did not survive the renderer swap, the app would render unstyled —
  // assert one computed value that can only come from that file.
  const bubbleBg = await timeline
    .locator('.msg-bubble')
    .first()
    .evaluate((el) => getComputedStyle(el).backgroundColor);
  expect(bubbleBg).not.toBe('rgba(0, 0, 0, 0)');

  expect(consoleErrors, `console errors: ${consoleErrors.join(' | ')}`).toEqual([]);
});
