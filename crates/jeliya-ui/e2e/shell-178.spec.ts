import { test, expect, type Page } from "@playwright/test";

// Shell, routing, and preference e2e coverage for #178 (§10.3 Verification).
// Exercises cross-boundary contracts that the unit/integration layer cannot cover:
//   URL ↔ WASM routing: canonicalization, malformed-URL recovery, in-app navigation
//   viewport width ↔ CSS/DOM: compact bottom-bar vs. medium/wide rail topology
//   browser localStorage ↔ WASM boot: legacy React key purge (AC-6)
//   room deep link ↔ room shell render (Route::Room typed destination)
// All tests run offline against the reproducible dist/ artifact — no daemon, no network.

let webSockets: string[] = [];
let apiRequests: string[] = [];

test.beforeEach(async ({ page, baseURL }) => {
  const origin = baseURL === undefined ? undefined : new URL(baseURL).origin;
  apiRequests = [];
  await page.route("**/*", (route) => {
    const url = new URL(route.request().url());
    if (origin !== undefined && url.origin === origin) {
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
  page.on("websocket", (ws) => {
    webSockets.push(ws.url());
  });
});

test.afterEach(() => {
  expect(
    webSockets,
    "offline smoke must see no WebSocket connections",
  ).toEqual([]);
  expect(
    apiRequests,
    "offline smoke must see no API-shaped same-origin traffic",
  ).toEqual([]);
});

// Wait for the deterministic mock to settle the shell to the Ready state.
async function waitForReady(page: Page): Promise<void> {
  await expect(page.locator("#status-footer")).toContainText("Connected", {
    timeout: 10_000,
  });
}

// ---- Route canonicalization (§5.C, D4) ------------------------------------

// The router replaces `/` with `/rooms` via history.replaceState at mount (D4):
// Back history has no bare-root entry, so the user cannot get stranded there.
test("bare / is canonicalized to /rooms via replace at mount", async ({
  page,
}) => {
  await page.goto("/");
  await waitForReady(page);
  expect(new URL(page.url()).pathname).toBe("/rooms");
});

// A strict Route::parse Err maps to Route::Rooms + navigate_replace (D7):
// the shell shows the rooms list and Back history has no garbage entry.
test("a malformed URL is recovered to /rooms via replace", async ({ page }) => {
  await page.goto("/not/a/valid/route");
  await waitForReady(page);
  expect(new URL(page.url()).pathname).toBe("/rooms");
});

// ---- In-app destination navigation (§5.A, §5.C) --------------------------

// Clicking the Fleet nav item pushes a new history entry at /fleet and renders
// the Fleet pane — proves the nav interaction → WebNavigation → URL roundtrip.
test("clicking the Fleet nav item navigates to /fleet", async ({ page }) => {
  await page.goto("/");
  await waitForReady(page);
  await page.locator("#nav-fleet").click();
  // Wait for the DOM to reflect the navigation before asserting the URL:
  // history.pushState fires synchronously in the WASM handler, so by the time
  // the Fleet pane is attached the URL is already /fleet.
  await expect(page.locator("#fleet-pane")).toBeAttached();
  expect(new URL(page.url()).pathname).toBe("/fleet");
});

// Clicking Settings from Fleet proves two sequential pushes in one session.
test("clicking the Settings nav item navigates to /settings", async ({
  page,
}) => {
  await page.goto("/");
  await waitForReady(page);
  await page.locator("#nav-fleet").click();
  await expect(page.locator("#fleet-pane")).toBeAttached();
  await page.locator("#nav-settings").click();
  await expect(page.locator("#settings-pane")).toBeAttached();
  expect(new URL(page.url()).pathname).toBe("/settings");
});

// ---- Room deep link (§5.C, §5.F) -----------------------------------------

// A direct load of a room-activity path renders the room shell without a
// redirect — the typed Route::Room { dest: Activity } path is already canonical.
// The fixture marker arms the scripted room list so the boot fold reaches Shell.
test("a room deep link renders the room shell", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("jeliya-e2e-boot-fixture", "1");
  });
  // ?rooms=1 scripts the mock to return one room row (fixture-room-0).
  await page.goto("/rooms/fixture-room-0/activity?rooms=1");
  await waitForReady(page);
  await expect(page.locator("#room-shell")).toBeAttached();
  await expect(page.locator("#room-header")).toBeAttached();
  await expect(page.locator("#room-dest-activity")).toBeAttached();
});

// ---- First-run onboarding reflects daemon truth (§5.D/§5.E, AC-1) --------

// The bootstrap fold renders onboarding from the scripted connection snapshot
// (D2), so the shell no longer skips first-run onboarding: a subject-`Absent`
// snapshot opens the identity step, a subject-`Present` snapshot with the
// scripted empty room.list opens the rooms step. The `?onboard=` fixture is
// marker-gated exactly like `?boot=`/`?rooms=`, so production never arms it and
// the ordinary fresh tab (no marker) still mounts the shell with the empty list.
test("an absent-subject snapshot opens the identity onboarding step", async ({
  page,
}) => {
  await page.addInitScript(() => {
    localStorage.setItem("jeliya-e2e-boot-fixture", "1");
  });
  await page.goto("/?onboard=identity");
  await expect(page.locator("#onboarding-identity")).toBeAttached();
  await expect(page.locator("#onboarding-create-identity")).toBeAttached();
  // Onboarding stands in for the shell: no global nav / rooms list is mounted.
  await expect(page.locator("#global-nav")).not.toBeAttached();
});

test("a present-subject snapshot with zero rooms opens the rooms onboarding step", async ({
  page,
}) => {
  await page.addInitScript(() => {
    localStorage.setItem("jeliya-e2e-boot-fixture", "1");
  });
  await page.goto("/?onboard=rooms");
  await expect(page.locator("#onboarding-rooms")).toBeAttached();
  await expect(page.locator("#onboarding-create-room")).toBeAttached();
});

// ---- Responsive nav topology (§8, COMPACT_MEDIA_QUERY/WIDE_MEDIA_QUERY) ---

// The compact shell (< 900px) places global destinations in a bottom bar.
// This pins the topology: a missing class silences the nav on phone-size
// system-WebViews while every desktop test stays green.
test.describe("compact viewport — bottom-bar nav topology", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("global nav has global-nav-bottom class at compact width", async ({
    page,
  }) => {
    await page.goto("/");
    await waitForReady(page);
    const nav = page.locator("#global-nav");
    await expect(nav).toHaveClass(/global-nav-bottom/);
    await expect(nav).not.toHaveClass(/global-nav-rail/);
  });
});

// The wide shell (≥ 1280px) places global destinations in a side rail.
test.describe("wide viewport — rail nav topology", () => {
  test.use({ viewport: { width: 1440, height: 900 } });

  test("global nav has global-nav-rail class at wide width", async ({
    page,
  }) => {
    await page.goto("/");
    await waitForReady(page);
    const nav = page.locator("#global-nav");
    await expect(nav).toHaveClass(/global-nav-rail/);
    await expect(nav).not.toHaveClass(/global-nav-bottom/);
  });
});

// ---- Legacy key purge (§6.5, AC-6) ---------------------------------------

// The boot-time WebPreferences purge must remove every legacy React localStorage
// key (exact names + the jeliya.draft.* prefix family) and leave none behind.
// This is the browser-side half of the write-only contract: the Rust integration
// test (shell_178.rs) scans source for getItem readers; this test confirms the
// removeItem calls actually fire against a live localStorage.
test("legacy React localStorage keys are purged at boot", async ({ page }) => {
  const legacyKeys = [
    "jeliya.lastRoom",
    "jeliya.aliases.v1",
    "jeliya.roomFlags",
    "jeliya.lastSeen",
    "jeliya.locale",
    "jeliya.draft.room-1", // prefix-family key
  ];
  await page.addInitScript((keys: string[]) => {
    for (const key of keys) {
      localStorage.setItem(key, "legacy-value");
    }
  }, legacyKeys);
  await page.goto("/");
  await waitForReady(page);
  for (const key of legacyKeys) {
    const value = await page.evaluate(
      (k: string) => localStorage.getItem(k),
      key,
    );
    expect(value, `legacy key "${key}" must be purged at boot`).toBeNull();
  }
});
