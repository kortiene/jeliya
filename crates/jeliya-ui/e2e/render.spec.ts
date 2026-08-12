import { test, expect } from "@playwright/test";

// The no-network render smoke (§11, Verification). It blocks every request
// outside the static server's own ORIGIN — a hostname-only check would let a
// regression open another loopback port (a daemon, some other local service)
// while the smoke stayed green — loads the reproducible artifact, and asserts
// the shared shell renders with the design system driving COMPUTED style,
// reproducing the #158 spike's "does the existing CSS survive the renderer
// swap" guard rather than trusting class presence. It runs offline and needs
// no daemon: #176 renders against the deterministic mock.
let webSockets: string[] = [];
let apiRequests: string[] = [];

test.beforeEach(async ({ page, baseURL }) => {
  const origin = baseURL === undefined ? undefined : new URL(baseURL).origin;
  apiRequests = [];
  await page.route("**/*", (route) => {
    const url = new URL(route.request().url());
    if (origin !== undefined && url.origin === origin) {
      // Same-origin does not mean anything goes: the static server carries
      // only the artifact's assets, and a request shaped like daemon API
      // traffic (/api/*, /ws) means the mock boundary leaked — the static
      // server would 404 it while the shell renders on, so the smoke would
      // stay silently green. Record and abort so the offending test fails
      // with the URL in hand; asset and SPA-route requests continue.
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
  // page.route() does not intercept WebSockets — the application's intended
  // transport is exactly that, so the no-network guarantee must cover it.
  // The #176 shell opens no socket at all: any attempt is a violation, so
  // every WebSocket is blocked (never connected upstream) AND recorded, and
  // the afterEach assertion fails the test that produced it.
  webSockets = [];
  await page.routeWebSocket(/.*/, () => {
    // Never connect: the socket stays dead.
  });
  page.on("websocket", (ws) => {
    webSockets.push(ws.url());
  });
});

test.afterEach(() => {
  expect(
    webSockets,
    "the offline smoke must see no WebSocket connections (no-network guarantee)",
  ).toEqual([]);
  expect(
    apiRequests,
    "the offline smoke must see no API-shaped same-origin traffic (mock boundary)",
  ).toEqual([]);
});

test("the Dioxus shell renders offline with the shared design system", async ({ page }) => {
  await page.goto("/");

  // The wasm module mounts the root; wait for the shell it renders.
  const root = page.locator("#app-root");
  await expect(root).toBeVisible();
  await expect(page.locator("#rooms-nav")).toBeAttached();
  await expect(page.locator("#main-content")).toBeAttached();
  await expect(page.locator("#center-empty")).toBeVisible();

  // Computed, not declared: the reused stylesheet must actually paint the
  // shell. The design system sets `body { background: var(--bg) }` (--bg is
  // #070d10); a transparent body means styles.css did not load. `.app` has no
  // explicit background (it inherits visually from body), so we check body.
  const background = await page.evaluate(
    () => getComputedStyle(document.body).backgroundColor,
  );
  expect(background).not.toBe("rgba(0, 0, 0, 0)");
  expect(background).not.toBe("transparent");
});

// The compose.rs mock drives the client through its full lifecycle: it calls
// `handle.start()`, sets `State::Ready`, and delivers all scripted mount reads
// in bounded cooperative passes (no wall clock). This test crosses the Rust
// mock → WASM → DOM boundary: if the compose.rs driver or the AppRoot
// lifecycle fold broke, the status footer would stay stuck before the Ready
// state's "Connected" label and
// the boot screen would never unmount.
test("the mock drives the shell to the Ready lifecycle state", async ({ page }) => {
  await page.goto("/");

  // After the mock settles, StatusFooter renders the catalog's Ready-state
  // copy: "client · Connected" (#177 moved the footer to catalog copy).
  // `toContainText` waits (default 5 s) so this handles the async WASM settle.
  const footer = page.locator("#status-footer");
  await expect(footer).toContainText("Connected");

  // The boot screen (rendered as the component ROOT while `!ready` in app.rs)
  // must be gone once the client is Ready: unmounted, not merely hidden.
  await expect(page.locator("#boot-screen")).not.toBeAttached();
});

// The daemon serves index.html as the SPA fallback for extension-less nested
// routes, where a `./`-relative asset URL would resolve under the route path
// (/rooms/jeliya_ui_web.js) and 404. The asset URLs are root-relative so the
// same document boots the app at every route; serve.mjs mirrors the daemon's
// fallback so this smoke is honest.
test("a nested SPA route loads the app via root-relative assets", async ({ page }) => {
  await page.goto("/rooms/r-99");

  const root = page.locator("#app-root");
  await expect(root).toBeVisible();
  await expect(page.locator("#status-footer")).toContainText("Connected");
});

// Compact viewports are a DIFFERENT rendering contract: at
// `max-width: 899.98px` the stylesheet hides every pane and the `pane-*`
// class on `.app` restores exactly one. Every other test runs the desktop
// viewport, where those rules never apply — so removing or misspelling the
// shell's `pane-rooms` class would blank the phone/system-WebView UI while
// the whole desktop suite stayed green. This smoke pins the compact
// contract: the sidebar pane (and its room list content — rows, or the
// loading/empty state) is visible, and the center pane is hidden.
test.describe("compact viewport", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("the compact shell shows the rooms pane and hides the center", async ({ page }) => {
    await page.goto("/");

    await expect(page.locator("#status-footer")).toContainText("Connected");
    await expect(page.locator("#rooms-nav")).toBeVisible();
    const roomsList = page.locator("#rooms-list");
    await expect(roomsList).toBeVisible();
    // The scripted mount read must SETTLE, not merely render something:
    // `#rooms-loading` shares the `rooms-empty` class, so a class-based
    // assertion would stay green if the room read never resolved and the
    // shipped shell sat on "Loading rooms…" forever. The mock's scripted
    // reply is the known empty account: the answered empty state must be
    // visible and the loading element gone.
    await expect(page.locator("#rooms-empty")).toBeVisible();
    await expect(page.locator("#rooms-loading")).not.toBeAttached();
    // `pane-rooms` shows ONLY the sidebar on compact viewports.
    await expect(page.locator("#main-content")).not.toBeVisible();
  });
});
