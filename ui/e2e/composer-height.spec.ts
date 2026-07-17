import { expect, test, MOCK_ROOMS } from './fixtures';

// Issue #57: the composer initializes while its pane is hidden on compact —
// it must still present a full one-line textarea on first reveal, grow
// multiline drafts to the 150px cap, keep drafts across hide/show, and
// re-measure on viewport changes.

const MULTILINE_DRAFT = Array.from({ length: 8 }, (_, i) => `draft line ${i + 1}`).join('\n');

test('the untouched composer shows a full one-line field before any keystroke', async ({
  app,
}) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  const textarea = app.composerTextarea;
  await expect(textarea).toBeVisible();
  await expect(textarea).toHaveAttribute('placeholder', `Message ${MOCK_ROOMS.main}`);
  // Nonzero content area before the first keystroke — the regression was a
  // clipped 0px strip measured inside the hidden pane.
  const box = await textarea.boundingBox();
  expect(box, 'composer textarea must be laid out').not.toBeNull();
  expect(box!.height).toBeGreaterThanOrEqual(24);
});

test('mobile: a multiline draft survives hide/show with its height restored', async ({
  app,
  compact,
}) => {
  test.skip(!compact, 'panes are only hidden on compact');
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  const textarea = app.composerTextarea;
  await textarea.fill(MULTILINE_DRAFT);
  const grownBox = await textarea.boundingBox();
  expect(grownBox!.height).toBeGreaterThan(80);

  // Hide the room pane, then reveal it again.
  await app.mobileTab('Rooms').click();
  await expect(app.center).toBeHidden();
  await app.roomItem(MOCK_ROOMS.main).click();

  // Draft preserved, height re-measured for the real layout — not a strip.
  await expect(textarea).toHaveValue(MULTILINE_DRAFT);
  await expect
    .poll(async () => (await textarea.boundingBox())!.height)
    .toBeGreaterThan(80);
});

test('multiline drafts grow to the cap without hiding the timeline', async ({ app }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  const textarea = app.composerTextarea;
  await textarea.fill(Array.from({ length: 40 }, (_, i) => `long line ${i + 1}`).join('\n'));

  // The textarea grows but stops at the 150px cap…
  const box = await textarea.boundingBox();
  expect(box!.height).toBeGreaterThan(100);
  expect(box!.height).toBeLessThanOrEqual(150);
  // …and its content stays reachable by scrolling inside the field.
  expect(await textarea.evaluate((el) => el.scrollHeight > el.clientHeight)).toBe(true);
  // The timeline is squeezed, never evicted (short viewports leave it only
  // a sliver, but it must stay laid out and visible).
  await expect(app.timeline).toBeVisible();
  const timelineBox = await app.timeline.boundingBox();
  expect(timelineBox!.height).toBeGreaterThan(0);
});

test('mobile: rotating the viewport recalculates the composer layout', async ({
  app,
  page,
  compact,
}) => {
  test.skip(!compact, 'rotation is a phone concern');
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  const textarea = app.composerTextarea;
  await textarea.fill(MULTILINE_DRAFT);
  await expect.poll(async () => (await textarea.boundingBox())!.height).toBeGreaterThan(80);

  const viewport = page.viewportSize()!;
  await page.setViewportSize({ width: viewport.height, height: viewport.width });

  await expect(textarea).toBeVisible();
  await expect(textarea).toHaveValue(MULTILINE_DRAFT);
  // Still a real, correctly measured field in the new geometry: taller than
  // one line, no taller than the cap.
  await expect
    .poll(async () => (await textarea.boundingBox())!.height)
    .toBeGreaterThan(40);
  expect((await textarea.boundingBox())!.height).toBeLessThanOrEqual(150);
});
