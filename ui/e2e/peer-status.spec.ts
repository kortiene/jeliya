import { expect, test, MOCK_ROOMS } from './fixtures';
import { en } from '../src/l10n/en';

// The room header's connectivity summary states what the daemon reported about
// peer reachability, and nothing else (docs/room-workbench.md, decision 4).
// It is the surface most tempted to round a fact up: it has one line to
// describe a whole room's worth of peers.

test('a connected peer with no known path yet is Connected, never Relay', async ({ app, page }) => {
  // PeerStatus.path is nullable while state is already `connected` — the SDK
  // knows a peer is reachable before it knows how. Guessing `relay` there
  // invents the exact fact (direct vs relay) the honesty rules protect.
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.design);

  const badge = page.locator('.p2p-badge');
  await expect(badge).toContainText('Connected');
  await expect(badge).not.toContainText('Relay');
  await expect(badge).not.toContainText('Direct');
});

test('a direct path is named, and a relay path is never hidden behind it', async ({ app, page, compact }) => {
  // The main fixture room has both a direct and a relay peer. Direct wins the
  // summary because it is the best real path the room has — but relay is still
  // reported as relay wherever it is the truth, which is the per-peer chips.
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.main);

  await expect(page.locator('.p2p-badge')).toContainText(en.roomHeaderPeerToPeer);

  // Compact keeps the chips behind the room-information disclosure, so the app
  // bar can hold a constant height; above compact the strip is already on
  // screen. Either way the relay peer is reachable and reads as relay.
  if (compact) await page.getByRole('button', { name: 'Room information' }).click();
  await expect(page.locator('.peer-chip.peer-path-relay').first()).toContainText('relay');
});

test('no peers connected says exactly that', async ({ app, page }) => {
  // Absence of an observed connection is not evidence of solitude: this room
  // has members whose peers are simply not connected.
  await app.gotoPopulated();
  await app.openRoom(MOCK_ROOMS.research);

  const badge = page.locator('.p2p-badge');
  await expect(badge).toContainText('No peers connected');
  await expect(badge).not.toContainText('Alone');
});
