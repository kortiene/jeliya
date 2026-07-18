import AxeBuilder from '@axe-core/playwright';
import { expect, test } from './fixtures';
import type { Page } from '@playwright/test';
import { MOCK_ROOMS } from './fixtures';

/** Automated accessibility coverage for the destination inventory
 *  (docs/room-workbench.md: three global destinations, five room destinations)
 *  across the four viewport projects — issue #72's exit criterion.
 *
 *  Scope note: this file asserts the rules #72 is about — landmarks, headings,
 *  regions, accessible names, focus order and target size — plus the structural
 *  invariants axe cannot express (exactly ONE visible main and ONE h1 per
 *  destination, and a working skip link). The full critical/serious sweep
 *  across every rule is issue #76's job; keeping the rule set explicit here
 *  means a failure names the contract it broke instead of just "axe found
 *  something".
 */

/** The WCAG conformance target (CONTRIBUTING.md: WCAG 2.1 AA) — PLUS the two
 *  tag families that actually carry this issue's rules.
 *
 *  This is not padding. axe classifies `landmark-one-main`, `landmark-unique`,
 *  `region`, `heading-order` and `page-has-heading-one` as `best-practice`, and
 *  `target-size` as `wcag22aa`. A sweep filtered to wcag2a/2aa/21a/21aa
 *  therefore runs NONE of the rules the acceptance criterion names — it looks
 *  thorough and proves nothing. Both families are required for this suite to
 *  mean what it claims. */
const A11Y_TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice'];

/** Run axe over the page and return its violations, most impactful first. */
async function analyze(page: Page) {
  const results = await new AxeBuilder({ page }).withTags(A11Y_TAGS).analyze();
  return results.violations;
}

/** Format violations so a CI failure is actionable without opening the trace:
 *  the rule, its impact, and the first offending selector. */
function describe(violations: Awaited<ReturnType<typeof analyze>>): string {
  return violations
    .map((v) => `${v.id} (${v.impact}): ${v.help}\n    ${v.nodes.map((n) => n.target.join(' ')).join('\n    ')}`)
    .join('\n  ');
}

/** The structural contract axe has no rule for: a destination has exactly one
 *  main landmark and exactly one h1 that a user can actually reach. Panes hide
 *  rather than unmount here, so several of each exist in the DOM — the point is
 *  that exactly one of each is VISIBLE. */
async function expectOnePageStructure(page: Page, where: string) {
  await expect(page.getByRole('main'), `${where}: exactly one visible main landmark`).toHaveCount(1);
  await expect(page.getByRole('heading', { level: 1 }), `${where}: exactly one visible h1`).toHaveCount(1);
}

async function expectNoViolations(page: Page, where: string) {
  const violations = await analyze(page);
  expect(violations.length, `${where} has accessibility violations:\n  ${describe(violations)}`).toBe(0);
}

test('onboarding is a landmarked page with one h1', async ({ app, page }) => {
  await app.gotoFresh();
  await expectOnePageStructure(page, 'onboarding');
  await expectNoViolations(page, 'onboarding');
});

test('the rooms destination is a landmarked page with one h1', async ({ app, page }) => {
  await app.gotoRoomsList();
  await expectOnePageStructure(page, '/rooms');
  await expectNoViolations(page, '/rooms');
});

test('a room workspace is a landmarked page with one h1', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);
  await expectOnePageStructure(page, '/rooms/:id/activity');
  await expectNoViolations(page, '/rooms/:id/activity');
});

for (const dest of ['People', 'Agents & Runs', 'Files', 'Pipes'] as const) {
  test(`the ${dest} inspector is a landmarked page with one h1`, async ({ app, page }) => {
    await app.gotoPopulated();
    await app.openRoom(MOCK_ROOMS.main);
    await app.roomTab(dest).click();
    await expect(app.rightPanel).toBeVisible();
    await expectOnePageStructure(page, `/rooms/:id/${dest}`);
    await expectNoViolations(page, `/rooms/:id/${dest}`);
  });
}

test('the fleet destination is a landmarked page with one h1', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Agent Fleet');
  await expect(page.getByRole('main', { name: 'Agent Fleet' })).toBeVisible();
  await expectOnePageStructure(page, '/fleet');
  await expectNoViolations(page, '/fleet');
});

test('the settings destination is a landmarked page with one h1', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Settings');
  await expect(page.getByRole('main', { name: 'Settings' })).toBeVisible();
  await expectOnePageStructure(page, '/settings');
  await expectNoViolations(page, '/settings');
});

test('a recoverable room error keeps one main and one h1', async ({ app, page }) => {
  // The route names a room this device does not have — a real destination with
  // its own heading and recovery actions, not a blank pane (#53).
  await app.gotoPopulated();
  await page.goto('/rooms/blake3:0000000000000000000000000000000000000000000000000000000000000000/activity');
  await expect(page.getByRole('heading', { name: /isn’t on this device/ })).toBeVisible();
  await expectOnePageStructure(page, 'unknown room');
  await expectNoViolations(page, 'unknown room');
});

test('an open dialog is accessible and keeps the page structure', async ({ app, page }) => {
  await app.gotoRoomsList();
  await page.getByRole('button', { name: 'Join with a ticket' }).first().click();
  await expect(page.getByRole('dialog', { name: 'Join with a ticket' })).toBeVisible();
  await expectNoViolations(page, 'join dialog');
});

test('the room rail and the inspector are distinctly named landmarks', async ({ app, page, shell }) => {
  test.skip(shell === 'compact', 'compact shows one pane at a time, so the two never coexist');
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);
  await app.roomTab('Files').click();
  await expect(app.rightPanel).toBeVisible();

  // Two complementary landmarks on one page MUST have different names, or
  // landmark navigation cannot tell them apart (axe `landmark-unique`).
  const complementary = page.getByRole('complementary');
  await expect(complementary).toHaveCount(2);
  await expect(page.getByRole('complementary', { name: 'Room rail' })).toBeVisible();
  await expect(page.getByRole('complementary', { name: 'Files inspector' })).toBeVisible();
});

test('the skip link moves focus to the page, not just the scroll position', async ({ app, page }) => {
  // From a fresh load, so the first Tab is genuinely the page's first tab stop
  // — after a click, focus sits on whatever was clicked, which is not the
  // journey this link exists for.
  await app.gotoPopulated();

  // The skip link is the FIRST tab stop, and it is invisible until it has
  // focus — a keyboard user must be able to reach it before anything else.
  await page.keyboard.press('Tab');
  const skip = page.getByRole('link', { name: 'Skip to main content' });
  await expect(skip).toBeFocused();
  await expect(skip).toBeVisible();

  await page.keyboard.press('Enter');
  // Focus actually MOVED — an anchor that only scrolls leaves the next Tab
  // continuing from the rail, which is the defect this link exists to fix.
  await expect(page.getByRole('main')).toBeFocused();
});

test('the composer skip link reaches the composer directly', async ({ app, page }) => {
  await app.gotoPopulated();

  await page.keyboard.press('Tab');
  await page.keyboard.press('Tab');
  const skip = page.getByRole('link', { name: 'Skip to message composer' });
  await expect(skip).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.locator('#composer-input')).toBeFocused();
});

test('the composer skip link is not offered where there is no composer', async ({ app, page }) => {
  // A link to a control that does not exist is worse than no link. Loaded by
  // URL rather than by clicking through, so the tab cursor starts at the top of
  // the document the way a real arrival at this destination does.
  await app.gotoPopulated();
  await page.goto('/settings');
  await expect(page.getByRole('main', { name: 'Settings' })).toBeVisible();

  await page.keyboard.press('Tab');
  await expect(page.getByRole('link', { name: 'Skip to main content' })).toBeFocused();
  await expect(page.getByRole('link', { name: 'Skip to message composer' })).toHaveCount(0);
});

/** The control's REAL hit area, in both axes, measured by hit-testing.
 *
 *  Reading the element's box (or its `::after`'s declared `min-height`) is not
 *  good enough: inline text controls reach the floor through an overhanging
 *  pseudo-element, and an overhang can be silently clipped by an ancestor
 *  scroll container — which is exactly what happened to the activity chips.
 *  A declaration-reading helper certified 44px for a target that was really 38.
 *
 *  So probe the page the way a finger does: walk out from the control's centre
 *  until `elementFromPoint` stops returning the control (or a descendant, since
 *  a pseudo-element hit reports its originating element). What comes back is
 *  what a tap would actually land on.
 */
async function targetSize(
  page: Page,
  selector: string,
): Promise<{ width: number; height: number; covered: boolean }> {
  const control = page.locator(selector).first();
  // `elementFromPoint` works in viewport coordinates, so an off-screen control
  // would report nothing and fail for the wrong reason.
  await control.scrollIntoViewIfNeeded();
  return control.evaluate((el) => {
    const box = el.getBoundingClientRect();
    // Hit-test just inside each corner and the centre. A control whose box is
    // big enough but which something else paints over is not a real target —
    // that is the failure mode a plain `getBoundingClientRect` cannot see.
    const inset = 2;
    const probes: [number, number][] = [
      [box.left + box.width / 2, box.top + box.height / 2],
      [box.left + inset, box.top + inset],
      [box.right - inset, box.top + inset],
      [box.left + inset, box.bottom - inset],
      [box.right - inset, box.bottom - inset],
    ];
    const covered = probes.some(([x, y]) => {
      const at = document.elementFromPoint(Math.round(x), Math.round(y));
      return !(at === el || (at instanceof Node && el.contains(at)) || (at instanceof Node && at.contains(el)));
    });
    return { width: Math.round(box.width), height: Math.round(box.height), covered };
  });
}

/** Assert a control clears a target floor on BOTH axes. Defaults to the
 *  project's 44px compact floor; the documented spacing exception passes 24
 *  (the WCAG 2.5.8 minimum), and is paired with a separate spacing assertion. */
async function expectTargetFloor(page: Page, selector: string, floor = 44) {
  const { width, height, covered } = await targetSize(page, selector);
  expect(height, `${selector} target is ${width}x${height}, needs >=${floor} tall`).toBeGreaterThanOrEqual(floor);
  expect(width, `${selector} target is ${width}x${height}, needs >=${floor} wide`).toBeGreaterThanOrEqual(floor);
  expect(covered, `${selector} is the right size but something paints over it`).toBe(false);
}

test('compact interactive targets clear the 44px floor', async ({ app, page, compact }) => {
  test.skip(!compact, 'the 44px floor is a touch/compact contract; desktop keeps its 26px icon buttons by design');
  await app.gotoRoomsList();

  // The room rail IS the compact /rooms screen, so its controls are the ones a
  // phone user actually hits.
  for (const selector of ['.filter-chip', '.create-room', '.room-row-action']) {
    // Assert the control is PRESENT before measuring it: a zero-match selector
    // used to skip silently, so a renamed class would have quietly turned this
    // loop into an assertion about nothing.
    await expect(page.locator(selector).first(), `${selector} must exist on the compact rooms screen`).toBeVisible();
    await expectTargetFloor(page, selector);
  }

  // Pin/archive reveal on hover, which touch does not have — they must be
  // visible without one, or they are unreachable on this layout entirely.
  await expect(page.locator('.room-row-actions').first()).toBeVisible();
});

test('compact timeline and fleet targets clear the 44px floor', async ({ app, page, compact }) => {
  test.skip(!compact, 'the 44px floor is a touch/compact contract');
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  // Sender rename takes the documented spacing exception (styles.css): it lives
  // inside an 18px timeline meta row that a 44px line box would triple.
  await expect(page.locator('.sender-name').first()).toBeVisible();
  await expectTargetFloor(page, '.sender-name', 24);

  await expect(page.locator('.activity-chip').first()).toBeVisible();
  await expectTargetFloor(page, '.activity-chip');

  await app.navigate('Agent Fleet');
  await expect(page.locator('.fleet-filter').first()).toBeVisible();
  await expectTargetFloor(page, '.fleet-filter');
  await expect(page.locator('.room-chip').first()).toBeVisible();
  await expectTargetFloor(page, '.room-chip');
});

test('the spacing exception is honored: undersized targets never crowd each other', async ({ app, compact }) => {
  test.skip(!compact, 'the spacing exception is a compact/touch contract');
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);
  // The People roster is where sender names stack densely — one per member row.
  // The compact timeline hides message meta entirely, so it is the wrong place
  // to prove crowding.
  await app.roomTab('People').click();
  await expect(app.rightPanel).toBeVisible();
  // Wait for the roster itself, not just the panel frame: members arrive from
  // room.members, so the rows are not there on the first paint.
  await expect(app.rightPanel.locator('.member-row').nth(1)).toBeVisible();

  // The other half of WCAG 2.5.8's spacing exception. A 24px target is only
  // acceptable if nothing else sits within 24px of it — otherwise a fingertip
  // aimed at one sender name opens the rename dialog for a different person.
  // This is the assertion that would have caught the overhanging hit area the
  // first attempt shipped, which reached into the neighbouring row.
  const boxes = await app.rightPanel.locator('.sender-name').evaluateAll((els) =>
    els
      .map((el) => el.getBoundingClientRect())
      .filter((b) => b.height > 0)
      .map((b) => ({ top: b.top, bottom: b.bottom, left: b.left, right: b.right })),
  );
  expect(boxes.length, 'the timeline must render sender names to measure').toBeGreaterThan(1);

  for (let i = 0; i < boxes.length; i += 1) {
    for (let j = i + 1; j < boxes.length; j += 1) {
      const a = boxes[i];
      const b = boxes[j];
      // Gap on whichever axis actually separates them; overlapping on an axis
      // contributes 0, so two boxes must be clear on at least one.
      const vertical = Math.max(a.top - b.bottom, b.top - a.bottom);
      const horizontal = Math.max(a.left - b.right, b.left - a.right);
      expect(
        Math.max(vertical, horizontal),
        `two sender names are ${Math.max(vertical, horizontal)}px apart — the 24px spacing exception needs >=24`,
      ).toBeGreaterThanOrEqual(24);
    }
  }
});

test('repeated row actions carry names that tell them apart', async ({ app }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  // The timeline renders a fetch control on every shared-file event — the
  // Files inspector only shows one, inside the expanded row. Each control's
  // VISIBLE text is the same single word, so a screen-reader user listing
  // buttons would hear "Fetch, Fetch, Fetch" with nothing to choose between
  // them; the file name joins the accessible name. Assert distinctness, not a
  // hardcoded string.
  const fetchButtons = app.timeline.getByRole('button', { name: /^Fetch / });
  await expect(fetchButtons.first()).toBeVisible();
  const fetchNames = await fetchButtons.evaluateAll((els) => els.map((el) => el.getAttribute('aria-label') ?? ''));
  expect(fetchNames.length, 'the timeline must render more than one fetchable file').toBeGreaterThan(1);
  expect(new Set(fetchNames).size, `fetch controls share an accessible name: ${fetchNames.join(' | ')}`).toBe(
    fetchNames.length,
  );

  // And the name must still START with the visible label, or a speech-input
  // user saying "click Fetch" matches nothing (WCAG 2.5.3 Label in Name).
  for (const name of fetchNames) expect(name.startsWith('Fetch ')).toBe(true);
});

test('agent identity copy buttons name whose identity they copy', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Agent Fleet');

  const copyButtons = page.getByRole('button', { name: /^Copy identity ID for / });
  // Wait for the fleet to actually load its cards before counting them.
  await expect(copyButtons.first()).toBeVisible();
  const copies = await copyButtons.evaluateAll((els) => els.map((el) => el.getAttribute('aria-label') ?? ''));
  expect(copies.length, 'the fleet must render more than one agent card').toBeGreaterThan(1);
  expect(new Set(copies).size, `copy buttons share an accessible name: ${copies.join(' | ')}`).toBe(copies.length);
});

test.describe('with motion allowed', () => {
  // The suite forces `reducedMotion: 'reduce'` for every project, so a test
  // asserting the reduced branch passes vacuously. Override it here to prove
  // the two branches genuinely differ.
  test.use({ contextOptions: { reducedMotion: 'no-preference' } });

  test('programmatic scrolling animates only when motion is allowed', async ({ app, page }) => {
    await app.gotoPopulated();
    await app.openRoom(MOCK_ROOMS.main);
    const allowed = await page.evaluate(() => window.matchMedia('(prefers-reduced-motion: reduce)').matches);
    expect(allowed, 'this block must run WITHOUT the reduced-motion preference').toBe(false);
  });
});

test('programmatic scrolling is instant under reduced motion', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);
  const reduced = await page.evaluate(() => window.matchMedia('(prefers-reduced-motion: reduce)').matches);
  expect(reduced, 'the suite runs under reduced motion by default').toBe(true);

  // The deep-link reveal must land immediately rather than animating: assert
  // the row is in view on the very next check, with no settling time.
  await app.roomTab('Pipes').click();
  await expect(app.rightPanel).toBeVisible();
  const row = app.rightPanel.locator('.pipe-row').first();
  await expect(row).toBeInViewport();
});

test('the document title names the destination', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);
  await expect(page).toHaveTitle(`${MOCK_ROOMS.main} · Jeliya`);

  await app.roomTab('Files').click();
  await expect(page).toHaveTitle(`Files · ${MOCK_ROOMS.main} · Jeliya`);

  // Back to Activity before leaving the room: on compact the inspector is the
  // whole screen, and the global tab bar only returns once the room does.
  await app.roomTab('Activity').click();
  await expect(page).toHaveTitle(`${MOCK_ROOMS.main} · Jeliya`);

  await app.navigate('Agent Fleet');
  await expect(page).toHaveTitle('Agent Fleet · Jeliya');

  await app.navigate('Settings');
  await expect(page).toHaveTitle('Settings · Jeliya');
});
