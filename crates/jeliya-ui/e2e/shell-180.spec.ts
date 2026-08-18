import { test, expect, type Page } from "@playwright/test";

// People / Agents / Fleet / Settings / Diagnostics e2e coverage for #180.
//
// All tests run offline against the reproducible dist/ artifact — no daemon,
// no network. The deterministic mock delivers scripted replies event-driven via
// pump_scripted_replies: each call parks until the app itself dispatches, then
// delivers, so there is no busy loop and no fixed pass count.
//
// Fixture arms:
//   ?rooms=1         — one fixture room (fixture-room-0, live=false, no caps)
//   ?rooms=1&caps=1  — same room with all five CapabilityTokens (D6 positive)
//
// Pump program budget per test (fresh page = fresh mock):
//   room.list → always consumed by AppRoot at mount
//   room.members + fleet.list + invite.list → consumed by PeoplePane
//   fleet.list → consumed by AgentsPane or FleetPane
//
// AC-4 (late-join #46 / expired-ticket re-invitation #47) is committed at the
// core/conformance layer; it is NOT a web e2e scenario until #171/#182 (R1).

let webSockets: string[] = [];
let apiRequests: string[] = [];

test.beforeEach(async ({ page, baseURL }) => {
  // Arm the fixture marker before any page load so compose.rs reads it on mount.
  await page.addInitScript(() =>
    localStorage.setItem("jeliya-e2e-boot-fixture", "1"),
  );
  // Track and abort any same-origin API/WS traffic; refuse all cross-origin.
  const origin = baseURL ? new URL(baseURL).origin : undefined;
  apiRequests = [];
  await page.route("**/*", (route) => {
    const url = new URL(route.request().url());
    if (origin && url.origin === origin) {
      const apiShaped =
        url.pathname.startsWith("/api/") ||
        url.pathname === "/ws" ||
        url.pathname.startsWith("/ws/");
      if (apiShaped) {
        apiRequests.push(url.pathname);
        return route.abort();
      }
      return route.continue();
    }
    return route.abort();
  });
  webSockets = [];
  await page.routeWebSocket(/.*/, () => {});
  page.on("websocket", (ws) => webSockets.push(ws.url()));
});

test.afterEach(() => {
  expect(webSockets, "offline suite: no WebSocket connections").toEqual([]);
  expect(apiRequests, "offline suite: no API-shaped traffic").toEqual([]);
});

async function waitForReady(page: Page): Promise<void> {
  await expect(page.locator("#status-footer")).toContainText("Connected", {
    timeout: 10_000,
  });
}

// ---- People: roster, invites, closed-room presence (D1) --------------------

test("People pane renders roster and invites from scripted reads", async ({
  page,
}) => {
  await page.goto("/rooms/fixture-room-0/people?rooms=1");
  await waitForReady(page);

  // Roster loads with both scripted members.
  const rosterList = page.locator("#roster-list");
  await expect(rosterList).toBeVisible({ timeout: 10_000 });
  await expect(
    rosterList.locator('[data-subject="blake3:fixture-authority"]'),
  ).toBeAttached();
  await expect(
    rosterList.locator('[data-subject="blake3:fixture-agent"]'),
  ).toBeAttached();

  // Invites load with both scripted rows (outstanding + expired).
  const invitesList = page.locator("#invites-list");
  await expect(invitesList).toBeVisible({ timeout: 5_000 });
  await expect(
    invitesList.locator('[data-invite="inv-outstanding"]'),
  ).toBeAttached();
  await expect(
    invitesList.locator('[data-invite="inv-expired"]'),
  ).toBeAttached();

  // Closed room (live=false): presence-unavailable note is shown, never a
  // fabricated peer count — membership and presence are two facts (D1).
  await expect(page.locator("#presence-summary")).toBeVisible();
  await expect(page.locator("#presence-closed-note")).toBeAttached();
});

// ---- People: capability-gated affordances absent vs present (D6) -----------

test("People: without capabilities, action affordances are absent not disabled (D6)", async ({
  page,
}) => {
  await page.goto("/rooms/fixture-room-0/people?rooms=1");
  await waitForReady(page);
  await expect(page.locator("#roster-list")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator("#invites-list")).toBeVisible({ timeout: 5_000 });

  // All action controls must be ABSENT (not disabled) with no capabilities.
  await expect(page.locator(".roster-remove")).not.toBeAttached();
  await expect(page.locator("#people-leave")).not.toBeAttached();
  await expect(page.locator("#invite-form")).not.toBeAttached();
  await expect(page.locator(".invite-revoke")).not.toBeAttached();
  await expect(page.locator(".invite-reinvite")).not.toBeAttached();
});

test("People: with capabilities, action affordances are present (D6)", async ({
  page,
}) => {
  await page.goto("/rooms/fixture-room-0/people?rooms=1&caps=1");
  await waitForReady(page);
  await expect(page.locator("#roster-list")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator("#invites-list")).toBeVisible({ timeout: 5_000 });

  // All action controls must appear when the room carries the authorizing tokens.
  await expect(page.locator(".roster-remove").first()).toBeAttached();
  await expect(page.locator("#people-leave")).toBeAttached();
  await expect(page.locator("#invite-form")).toBeAttached();
  await expect(page.locator(".invite-revoke").first()).toBeAttached();
  await expect(page.locator(".invite-reinvite").first()).toBeAttached();
});

// ---- People: ConfirmDialog focus-safety (D4) --------------------------------

test("ConfirmDialog: Cancel before Confirm in DOM order, room disambiguator present (D4)", async ({
  page,
}) => {
  await page.goto("/rooms/fixture-room-0/people?rooms=1&caps=1");
  await waitForReady(page);

  const leaveBtn = page.locator("#people-leave");
  await expect(leaveBtn).toBeAttached({ timeout: 10_000 });
  await leaveBtn.click();

  // The dialog body and the room short-id disambiguator both appear (D4 rule 6).
  await expect(page.locator("#confirm-body")).toBeVisible({ timeout: 5_000 });
  // "fixture-room-0" → last colon-segment = "fixture-room-0", take 8 → "fixture-"
  await expect(page.locator("#confirm-room-short")).toContainText("fixture-");

  // Cancel (#confirm-cancel) must come BEFORE Confirm (#confirm-accept) in the
  // DOM / tab order — the first interactive control reached by Tab is the safe one.
  const order = await page.evaluate(() => {
    const parent = document.querySelector(".confirm-actions");
    if (!parent) return null;
    const ids = Array.from(parent.children).map(
      (el) => (el as HTMLElement).id,
    );
    return {
      cancel: ids.indexOf("confirm-cancel"),
      accept: ids.indexOf("confirm-accept"),
    };
  });
  expect(order, "confirm-actions must contain both buttons").not.toBeNull();
  expect(
    order!.cancel,
    "Cancel must precede Confirm in DOM order (D4)",
  ).toBeLessThan(order!.accept);
});

// ---- People: item deep link stability (AC-1) --------------------------------

test("People item deep link selects the member row (AC-1)", async ({ page }) => {
  await page.goto(
    "/rooms/fixture-room-0/people/blake3%3Afixture-authority?rooms=1",
  );
  await waitForReady(page);
  await expect(page.locator("#roster-list")).toBeVisible({ timeout: 10_000 });

  // The linked member row gets the "selected" CSS class; all other rows do not.
  const row = page.locator('[data-subject="blake3:fixture-authority"]');
  await expect(row).toHaveClass(/selected/);
  const otherRow = page.locator('[data-subject="blake3:fixture-agent"]');
  await expect(otherRow).not.toHaveClass(/selected/);
});

// ---- Agents: list + item deep link (AC-1) -----------------------------------

test("Agents pane renders agent list from scripted fleet read", async ({
  page,
}) => {
  await page.goto("/rooms/fixture-room-0/agents?rooms=1");
  await waitForReady(page);

  await expect(page.locator("#agents-list")).toBeVisible({ timeout: 10_000 });
  // The scripted fixture-agent appears in the list.
  await expect(
    page.locator('[data-subject="blake3:fixture-agent"]'),
  ).toBeAttached();
});

test("Agents item deep link marks row expanded and renders run-history (AC-1)", async ({
  page,
}) => {
  await page.goto(
    "/rooms/fixture-room-0/agents/blake3%3Afixture-agent?rooms=1",
  );
  await waitForReady(page);
  await expect(page.locator("#agents-list")).toBeVisible({ timeout: 10_000 });

  // The selected agent's toggle button reports aria-expanded="true".
  const toggleBtn = page.locator(
    '[data-subject="blake3:fixture-agent"] .agent-select',
  );
  await expect(toggleBtn).toHaveAttribute("aria-expanded", "true");

  // #run-history is rendered (Loading state — status.history is not scripted,
  // but history=Some(Loading) is set before the await so the section appears).
  await expect(page.locator("#run-history")).toBeAttached({ timeout: 5_000 });
});

// ---- Fleet: render + filter toggle + row navigation (D7) -------------------

test("Fleet pane renders both scripted agents", async ({ page }) => {
  await page.goto("/fleet?rooms=1");
  await waitForReady(page);

  await expect(page.locator("#fleet-pane")).toBeVisible({ timeout: 10_000 });
  // Both fixture rows (Working + Stale) appear under the All filter.
  await expect(page.locator(".fleet-row")).toHaveCount(2, { timeout: 10_000 });
});

test("Fleet Live filter toggles aria-pressed correctly", async ({ page }) => {
  await page.goto("/fleet?rooms=1");
  await waitForReady(page);
  await expect(page.locator(".fleet-row").first()).toBeAttached({
    timeout: 10_000,
  });

  // All filter is active by default.
  await expect(page.locator("#fleet-filter-all")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.locator("#fleet-filter-live")).toHaveAttribute(
    "aria-pressed",
    "false",
  );

  // Switching to Live toggles both buttons.
  await page.locator("#fleet-filter-live").click();
  await expect(page.locator("#fleet-filter-live")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.locator("#fleet-filter-all")).toHaveAttribute(
    "aria-pressed",
    "false",
  );
});

test("clicking a Fleet row navigates to that room's Agents pane", async ({
  page,
}) => {
  await page.goto("/fleet?rooms=1");
  await waitForReady(page);
  await expect(page.locator(".fleet-select").first()).toBeAttached({
    timeout: 10_000,
  });

  await page.locator(".fleet-select").first().click();
  // The click pushes Route::Room { dest: Agents } via History API.
  await expect(page).toHaveURL(/\/rooms\/fixture-room-0\/agents/, {
    timeout: 5_000,
  });
});

// ---- Settings: sections present + diagnostics bounded field set (AC-6) -----

test("Settings pane has all five expected sections", async ({ page }) => {
  await page.goto("/settings");
  await waitForReady(page);
  await expect(page.locator("#settings-pane")).toBeVisible({ timeout: 10_000 });

  await expect(page.locator("#settings-identity")).toBeAttached();
  await expect(page.locator("#settings-language")).toBeAttached();
  await expect(page.locator("#settings-self-label")).toBeAttached();
  await expect(page.locator("#settings-aliases")).toBeAttached();
  await expect(page.locator("#settings-diagnostics")).toBeAttached();
});

test("diagnostics card has bounded field set, redaction note, and copy action (AC-6)", async ({
  page,
}) => {
  await page.goto("/settings");
  await waitForReady(page);
  await expect(page.locator("#settings-pane")).toBeVisible({ timeout: 10_000 });

  // The card is present with the lifecycle state and error detail fields.
  await expect(page.locator("#diagnostics-card")).toBeAttached();
  await expect(page.locator("#diagnostics-card-state")).not.toBeEmpty();
  await expect(page.locator("#diagnostics-card-detail")).toBeAttached();

  // The redaction note and the copy action are both present.
  await expect(page.locator("#diagnostics-redaction")).toBeAttached();
  await expect(page.locator("#diagnostics-copy")).toBeAttached();

  // Capability tokens must not appear in the bounded field set (AC-6).
  await expect(page.locator("#diagnostics-card")).not.toContainText(
    "InviteMint",
  );
  await expect(page.locator("#diagnostics-card")).not.toContainText("RoomLeave");
});
