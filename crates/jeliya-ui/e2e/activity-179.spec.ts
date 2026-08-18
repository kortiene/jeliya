import { test, expect, type Page } from "@playwright/test";

// Activity pane, composer, filter chips, and draft-persistence e2e coverage
// for #179 (§10 Verification / AC-1 through AC-6).
// Crosses system boundaries the host test layer cannot reach:
//   Reconciler state → DOM: loading notice when no view has converged (D7)
//   Capability gate → DOM: composer vs. read-only floor (D8)
//   Filter chip interaction: aria-pressed toggle, multi-select
//   Composer keyboard/viewport contract: Enter hint on desktop, absent on compact
//   Per-room draft: sessionStorage ↔ WASM ↔ DOM across in-app route changes
// All tests run offline against the reproducible dist/ — no daemon, no network.

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

async function waitForReady(page: Page): Promise<void> {
  await expect(page.locator("#status-footer")).toContainText("Connected", {
    timeout: 10_000,
  });
}

// Navigate to a room's Activity destination using the marker-gated room fixture
// (?rooms=1 populates fixture-room-0 with MessageSend capability, marker arms
// the fixture). Waits for the settled shell before returning.
async function gotoRoomActivity(page: Page): Promise<void> {
  await page.addInitScript(() => {
    localStorage.setItem("jeliya-e2e-boot-fixture", "1");
  });
  await page.goto("/rooms/fixture-room-0/activity?rooms=1");
  await waitForReady(page);
  // The room shell must be attached before any pane assertions.
  await expect(page.locator("#room-shell")).toBeAttached();
}

// ---- Activity pane structure (§7, §9) ----------------------------------------

// The Activity destination for a reachable room renders the full pane structure:
// filters, timeline scroller, and the composer. This is the real #179 content,
// not the "#178 skeleton" placeholder the shell previously showed.
test("the Activity pane renders with timeline and composer for a capable room", async ({
  page,
}) => {
  await gotoRoomActivity(page);

  // The real Activity pane — not the skeleton — must be mounted.
  await expect(page.locator("#activity-pane")).toBeAttached();
  // The scroller and its stick-to-bottom anchor are the scroll contract (§7 AC).
  await expect(page.locator("#activity-timeline")).toBeAttached();
  await expect(page.locator("#activity-bottom")).toBeAttached();
  // The five view-only activity filters render in the default all-visible state.
  await expect(page.locator("#activity-filters")).toBeAttached();
  // The composer renders: fixture-room-0 has the MessageSend capability (D8).
  await expect(page.locator("#composer")).toBeAttached();
  await expect(page.locator("#composer-input")).toBeAttached();
  await expect(page.locator("#composer-send")).toBeAttached();
  // The attachment handoff degrades to an honest disabled affordance until #181.
  const attach = page.locator("#composer-attach");
  await expect(attach).toBeAttached();
  await expect(attach).toBeDisabled();
  // The read-only floor is NOT shown (the room has MessageSend capability).
  await expect(page.locator("#composer-readonly")).not.toBeAttached();
});

// The Activity timeline is in loading state before the reconciler converges a
// view — the mock does not script any reconciler replies, so the pane renders
// the honest "Loading" notice, never an empty timeline (D7: "booting is unknown,
// not zero"). The empty notice requires an answered view with no events, which
// the mock's silence deliberately never provides.
test("the Activity timeline shows loading state before the reconciler converges", async ({
  page,
}) => {
  await gotoRoomActivity(page);

  // Loading notice is attached; the empty state is NOT (D7: unknown ≠ empty).
  await expect(page.locator("#activity-loading")).toBeAttached();
  await expect(page.locator("#activity-empty")).not.toBeAttached();
  // No timeline rows exist yet (no converged view).
  await expect(page.locator(".timeline-row")).toHaveCount(0);
  // The new-activity affordance requires items to count; it must be absent.
  await expect(page.locator("#activity-new")).not.toBeAttached();
  // The resync notice is absent in the initial quiet state.
  await expect(page.locator("#activity-resyncing")).not.toBeAttached();
});

// ---- Room destination strip (§5.F, room_shell.rs) ----------------------------

// The room navigation strip carries exactly five destination tabs; Activity is
// selected (aria-selected="true") since that is the routed destination.
test("the room destination strip renders five tabs with Activity selected", async ({
  page,
}) => {
  await gotoRoomActivity(page);

  await expect(page.locator("#room-dest-activity")).toBeAttached();
  await expect(page.locator("#room-dest-people")).toBeAttached();
  await expect(page.locator("#room-dest-agents")).toBeAttached();
  await expect(page.locator("#room-dest-files")).toBeAttached();
  await expect(page.locator("#room-dest-pipes")).toBeAttached();

  // Activity is the routed destination; its tab is selected.
  await expect(page.locator("#room-dest-activity")).toHaveAttribute(
    "aria-selected",
    "true",
  );
  // Every other tab is not selected.
  for (const id of [
    "#room-dest-people",
    "#room-dest-agents",
    "#room-dest-files",
    "#room-dest-pipes",
  ]) {
    await expect(page.locator(id)).toHaveAttribute("aria-selected", "false");
  }
});

// ---- Activity filters (§1, D1) -----------------------------------------------

// The five view-only activity filters (Conversation / Agent runs / Membership /
// Files / Pipes) render and none is pressed in the default all-visible state
// (empty filter set = every signed event is projected — D1).
test("the five activity filter chips render and are initially unpressed", async ({
  page,
}) => {
  await gotoRoomActivity(page);

  const chips = page.locator(".filter-chip");
  await expect(chips).toHaveCount(5);
  // None of the filters is active by default.
  for (const chip of await chips.all()) {
    await expect(chip).toHaveAttribute("aria-pressed", "false");
    await expect(chip).not.toHaveClass(/selected/);
  }
});

// Clicking a filter chip toggles aria-pressed; clicking it again deselects it.
// Filters are REVERSIBLE view state layered on the signed timeline (D1): toggling
// a filter never drops signed events from the underlying projection.
test("clicking a filter chip toggles aria-pressed and can be deselected", async ({
  page,
}) => {
  await gotoRoomActivity(page);

  const first = page.locator(".filter-chip").first();
  // Initial state: unpressed.
  await expect(first).toHaveAttribute("aria-pressed", "false");

  // First click: pressed.
  await first.click();
  await expect(first).toHaveAttribute("aria-pressed", "true");
  await expect(first).toHaveClass(/selected/);

  // Second click: back to unpressed (deselected).
  await first.click();
  await expect(first).toHaveAttribute("aria-pressed", "false");
  await expect(first).not.toHaveClass(/selected/);
});

// Multiple filter chips can be active at the same time (multi-select contract,
// §1). Selecting two chips leaves the other three untouched.
test("multiple filter chips can be active simultaneously", async ({ page }) => {
  await gotoRoomActivity(page);

  const chips = page.locator(".filter-chip");
  const first = chips.nth(0);
  const second = chips.nth(1);

  await first.click();
  await second.click();

  await expect(first).toHaveAttribute("aria-pressed", "true");
  await expect(second).toHaveAttribute("aria-pressed", "true");
  // The remaining three are untouched.
  await expect(chips.nth(2)).toHaveAttribute("aria-pressed", "false");
  await expect(chips.nth(3)).toHaveAttribute("aria-pressed", "false");
  await expect(chips.nth(4)).toHaveAttribute("aria-pressed", "false");
});

// ---- Composer keyboard contract (§8) -----------------------------------------

// On a wide/desktop viewport the composer shows an Enter-to-send hint: the Shell
// is Wide, `uses_bottom_bar()` returns false, so the hint renders (§8). This
// proves the Shell injection propagates from web_shell() through AppRoot into
// the Composer component.
test("the desktop composer shows the Enter-to-send hint", async ({ page }) => {
  await gotoRoomActivity(page);
  await expect(page.locator("#composer-hint")).toBeAttached();
});

// ---- Compact viewport — composer keyboard contract (§8) ----------------------

test.describe("compact viewport", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  // On a compact viewport (< 900px) web_shell() returns Shell::Compact,
  // uses_bottom_bar() returns true, and the Enter-to-send hint is NOT rendered
  // (Enter inserts a newline; the explicit send button is the only send path).
  // The composer itself still renders — only the hint is suppressed.
  test("the compact composer omits the Enter-to-send hint", async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.setItem("jeliya-e2e-boot-fixture", "1");
    });
    await page.goto("/rooms/fixture-room-0/activity?rooms=1");
    await waitForReady(page);
    // The Activity pane is in the DOM (route is Room Activity) even if the CSS
    // hides the center panel on compact. Use toBeAttached for pane assertions.
    await expect(page.locator("#room-shell")).toBeAttached();
    await expect(page.locator("#activity-pane")).toBeAttached();
    // The hint is NOT rendered when the compact shell is active.
    await expect(page.locator("#composer-hint")).not.toBeAttached();
    // The composer itself is still present (can_send = true).
    await expect(page.locator("#composer")).toBeAttached();
    await expect(page.locator("#composer-send")).toBeAttached();
  });
});

// ---- Per-room draft persistence across route changes (§1, §8 AC) -----------

// Typing in the composer writes the draft to sessionStorage-backed WebPreferences
// (jeliya.dx.v1 namespace). Navigating away unmounts the ActivityPane; navigating
// back remounts it and seeds the textarea from the stored draft. This crosses the
// UI → sessionStorage → UI boundary — the "drafts survive route changes" AC (§8)
// that no unit test can cover.
test("a typed draft survives an in-app round-trip through another global destination", async ({
  page,
}) => {
  await gotoRoomActivity(page);

  // Type a draft into the composer. `.click()` focuses, then keyboard.type()
  // fires real key events that Dioxus's oninput handler picks up.
  await page.locator("#composer-input").click();
  await page.keyboard.type("draft text for round-trip");
  // Confirm the text is in the textarea before navigating away.
  await expect(page.locator("#composer-input")).toHaveValue(
    "draft text for round-trip",
  );

  // Navigate away to Fleet — the ActivityPane unmounts, saving the reading
  // position and calling deactivate_room (§7 AC).
  await page.locator("#nav-fleet").click();
  await expect(page.locator("#fleet-pane")).toBeAttached();
  await expect(page.locator("#activity-pane")).not.toBeAttached();

  // Navigate back to Rooms, then click the fixture room to open Activity again.
  await page.locator("#nav-rooms").click();
  await expect(page.locator("#rooms-nav")).toBeVisible();
  // The rooms list populated at boot (via the ?rooms=1 fixture) shows the room.
  await page.locator(".room-select").first().click();

  // The ActivityPane remounts; it seeds the textarea from the persisted draft.
  await expect(page.locator("#activity-pane")).toBeAttached();
  await expect(page.locator("#composer-input")).toHaveValue(
    "draft text for round-trip",
  );
});
