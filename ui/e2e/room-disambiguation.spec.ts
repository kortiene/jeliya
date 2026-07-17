import { expect, test, MOCK_ROOMS, HOMONYM_ROOM } from './fixtures';
import type { Page } from '@playwright/test';

// Issue #49 / docs/room-workbench.md decision 6: `room_id` is identity, `name`
// is a non-unique label. Any surface where acting on the wrong room matters
// shows the short room id. The fixture seeds two rooms both named "Bug Triage"
// (ui/src/lib/mock.ts) so the disambiguation has something to disambiguate.

/** shortId(id) renders the tail of a `blake3:<hex>` id as `abcd…wxyz`. */
const SHORT_ID = /^[0-9a-f]{4}…[0-9a-f]{4}$/;

function dialog(page: Page) {
  return page.getByRole('dialog');
}

test('homonymous rooms each show a distinct short id in the rail', async ({ app, page }) => {
  await app.gotoRoomsList();

  const rows = app.roomItem(HOMONYM_ROOM);
  await expect(rows).toHaveCount(2);

  // Every homonym row carries its own short-id token…
  const disambigs = rows.locator('.room-disambig');
  await expect(disambigs).toHaveCount(2);
  const ids = (await disambigs.allInnerTexts()).map((t) => t.trim());
  expect(ids[0]).toMatch(SHORT_ID);
  expect(ids[1]).toMatch(SHORT_ID);
  // …and the two differ — that difference is the whole point.
  expect(ids[0]).not.toBe(ids[1]);

  // The short id is real text in the row, so it lands in the accessible name:
  // narrowing the same locator by it resolves to exactly one row.
  await expect(app.roomItem(HOMONYM_ROOM, ids[0])).toHaveCount(1);

  // A uniquely-named room gets no disambiguator — it would be noise there.
  await expect(app.roomItem(MOCK_ROOMS.main).locator('.room-disambig')).toHaveCount(0);
  // And nothing about this reintroduces the retired "Active" label.
  await expect(page.getByRole('navigation', { name: 'Rooms' })).not.toContainText('Active');
});

test('the leave dialog always shows the room short id, even for a unique name', async ({
  app,
  page,
}) => {
  // Product Review has a unique name, so its rail row shows no disambiguator —
  // but leaving is destructive and irreversible, so the dialog shows the short
  // id anyway (decision 6: destructive actions repeat it, homonym or not).
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.review);
  await app.goToRoomDest('People');
  await app.rightPanel.getByRole('button', { name: 'Leave', exact: true }).click();

  const leave = dialog(page);
  await expect(leave).toBeVisible();
  await expect(leave.locator('.room-disambig')).toHaveCount(1);
  await expect(leave.locator('.room-disambig')).toHaveText(SHORT_ID);
});

test('leaving one homonym shows that exact room’s short id', async ({ app, page }) => {
  await app.gotoRoomsList();

  // The first "Bug Triage" row is the one this identity is a plain member of
  // (mock insertion order), so its roster offers Leave. Read its short id off
  // the row before opening it.
  const memberRow = app.roomItem(HOMONYM_ROOM).first();
  const rowId = (await memberRow.locator('.room-disambig').innerText()).trim();
  expect(rowId).toMatch(SHORT_ID);

  await memberRow.click();
  await expect(page.getByRole('heading', { level: 1, name: HOMONYM_ROOM })).toBeVisible();
  await expect(app.timeline).toBeVisible();

  await app.goToRoomDest('People');
  await app.rightPanel.getByRole('button', { name: 'Leave', exact: true }).click();

  // The dialog's short id is the one from the row we opened — not the other
  // same-named room's. That is exactly what stops a departure landing on the
  // wrong room.
  await expect(dialog(page).locator('.room-disambig')).toHaveText(rowId);
});

test('the fleet shows short ids only on homonymous room chips', async ({ app, page }) => {
  await app.gotoPopulated();
  await app.navigate('Agent Fleet');

  const fleet = page.getByRole('region', { name: 'Agent Fleet' });
  const qaCard = fleet.locator('.fleet-card', { hasText: 'QA Agent' });
  await expect(qaCard).toHaveCount(1);

  // QA is a member of both "Bug Triage" rooms — two identically-labelled chips
  // that only the short id separates.
  const triageChips = qaCard.locator('.room-chip', { hasText: HOMONYM_ROOM });
  await expect(triageChips).toHaveCount(2);
  await expect(triageChips.locator('.room-disambig')).toHaveCount(2);

  // Its uniquely-named room chip carries none.
  const mvpChip = qaCard.locator('.room-chip', { hasText: MOCK_ROOMS.main });
  await expect(mvpChip).toHaveCount(1);
  await expect(mvpChip.locator('.room-disambig')).toHaveCount(0);
});

test('creating a room warns on a local name collision without blocking', async ({ app, page }) => {
  await app.gotoRoomsList();
  await page.getByRole('button', { name: 'Create Room', exact: true }).click();

  const create = dialog(page);
  const input = create.getByLabel('Room name');
  const submit = create.getByRole('button', { name: 'Create room' });
  const warning = create.getByText('already exists on this device', { exact: false });

  // No warning for a fresh name.
  await input.fill('A Brand New Room');
  await expect(warning).toHaveCount(0);
  await expect(submit).toBeEnabled();

  // A collision — folded the same way homonym detection folds (trim + case) —
  // warns but never disables Create.
  await input.fill(`  ${MOCK_ROOMS.design.toUpperCase()}  `);
  await expect(warning).toBeVisible();
  await expect(submit).toBeEnabled();

  // Clearing the collision clears the warning.
  await input.fill('Something Else Entirely');
  await expect(warning).toHaveCount(0);
});
