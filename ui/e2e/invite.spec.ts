import { expect, test, MOCK_ROOMS } from './fixtures';
import type { AppDriver } from './fixtures';
import type { Page } from '@playwright/test';

// Issue #66 (P14): the guided invitation + re-invitation flow. Identity is
// validated inline before submit; the agent role warns; expiry is a preset or a
// custom value; minting shows the combined ticket#address with Copy and a live
// lifecycle chip; a lapsed ticket flips to Expired and offers Invite Again.
//
// The waiting→joined LIVE flip is cert-gated on Rust #46/#47, so it is not
// forced here — the Joined *rendering* given an active roster is covered by the
// P13 inviteState fixtures (ui/src/lib/invite.test.ts). What this spec drives is
// everything the React flow owns end to end.

// A well-formed bare 64-hex identity id, and a same-length non-hex one that must
// still be rejected (length alone is not validity).
const GOOD_ID = 'a'.repeat(64);
const BAD_HEX = 'g'.repeat(64);

function modal(page: Page) {
  return page.getByRole('dialog');
}

/** Open the Invite dialog from the room header — one button on every shell
 *  (the app bar carries it on compact, the header on medium/wide). */
async function openInvite(app: AppDriver): Promise<void> {
  await app.openRoom(MOCK_ROOMS.main);
  await app.page.getByRole('button', { name: 'Invite', exact: true }).click();
  await expect(modal(app.page)).toBeVisible();
}

test('inline identity validation gates the submit', async ({ app, page }) => {
  await app.gotoPopulated();
  await openInvite(app);
  const dlg = modal(page);
  const submit = dlg.getByRole('button', { name: 'Generate ticket' });

  // Empty: help state, submit disabled — never fire an obviously bad id.
  await expect(submit).toBeDisabled();

  // A malformed id keeps submit disabled and surfaces an inline error.
  await dlg.getByLabel('Invitee identity id').fill(BAD_HEX);
  await expect(submit).toBeDisabled();
  await expect(dlg.getByText(/exactly 64 hexadecimal characters/)).toBeVisible();

  // A valid 64-hex id clears the error and enables submit.
  await dlg.getByLabel('Invitee identity id').fill(GOOD_ID);
  await expect(dlg.getByText(/exactly 64 hexadecimal characters/)).toHaveCount(0);
  await expect(submit).toBeEnabled();
});

test('a preset expiry mints the combined ticket#address with Copy and a Waiting chip', async ({
  app,
  page,
}) => {
  await app.gotoPopulated();
  await openInvite(app);
  const dlg = modal(page);

  await dlg.getByLabel('Invitee identity id').fill(GOOD_ID);
  await dlg.getByRole('button', { name: '24 hours' }).click();
  await dlg.getByRole('button', { name: 'Generate ticket' }).click();

  // The combined invite is ticket#address, built via buildCombinedInvite.
  const combined = dlg.getByRole('textbox', { name: 'Combined invite (ticket and peer address)' });
  await expect(combined).toHaveValue(/roomtkt1.+#.+@127\.0\.0\.1:52731/);
  await expect(dlg.getByRole('button', { name: 'Copy invite' })).toBeVisible();

  // The lifecycle chip reads Waiting — the roster has no active row yet.
  await expect(dlg.locator('.chip-label')).toHaveText('Waiting');

  // A scannable QR of the SAME combined invite renders alongside Copy (#103).
  await expect(dlg.getByRole('img', { name: /QR code for the room invite/ })).toBeVisible();
});

test('a custom expiry mints a ticket', async ({ app, page }) => {
  await app.gotoPopulated();
  await openInvite(app);
  const dlg = modal(page);

  await dlg.getByLabel('Invitee identity id').fill(GOOD_ID);
  await dlg.getByText('Advanced / custom expiry').click();
  await dlg.getByLabel('Custom expiry seconds').fill('120');
  await dlg.getByRole('button', { name: 'Generate ticket' }).click();

  await expect(
    dlg.getByRole('textbox', { name: 'Combined invite (ticket and peer address)' }),
  ).toBeVisible();
  await expect(dlg.locator('.chip-label')).toHaveText('Waiting');
});

test('the agent role reveals a security warning', async ({ app, page }) => {
  await app.gotoPopulated();
  await openInvite(app);
  const dlg = modal(page);

  // Member (default): no warning.
  await expect(dlg.getByRole('alert')).toHaveCount(0);

  // Agent: the role="alert" security warning appears, matching the Add-Agent tone.
  await dlg.getByRole('radio', { name: /Agent/ }).check();
  await expect(dlg.getByRole('alert')).toBeVisible();
  await expect(dlg.getByText(/arbitrary code \/ file execution/)).toBeVisible();

  // Back to member: the warning is gone.
  await dlg.getByRole('radio', { name: /Member/ }).check();
  await expect(dlg.getByRole('alert')).toHaveCount(0);
});

test('a lapsed ticket flips to Expired and Invite Again re-mints', async ({ app, page }) => {
  await app.gotoPopulated();
  await openInvite(app);
  const dlg = modal(page);

  await dlg.getByLabel('Invitee identity id').fill(GOOD_ID);
  // A one-second custom expiry so the live tick flips waiting→expired without a
  // reload — the transition the flow owns, driven for real.
  await dlg.getByText('Advanced / custom expiry').click();
  await dlg.getByLabel('Custom expiry seconds').fill('1');
  await dlg.getByRole('button', { name: 'Generate ticket' }).click();

  const combined = dlg.getByRole('textbox', { name: 'Combined invite (ticket and peer address)' });
  await expect(combined).toBeVisible();
  const firstTicket = await combined.inputValue();

  // The chip flips to Expired on its own, and Invite Again appears.
  await expect(dlg.locator('.chip-label')).toHaveText('Expired', { timeout: 8000 });
  const again = dlg.getByRole('button', { name: 'Invite again' });
  await expect(again).toBeVisible();

  // Re-minting yields a fresh ticket (a new roomtkt1 token).
  await again.click();
  await expect(combined).not.toHaveValue(firstTicket, { timeout: 5000 });
});
