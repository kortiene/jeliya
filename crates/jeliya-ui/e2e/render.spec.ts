import { test, expect } from "@playwright/test";

// The no-network render smoke (§11, Verification). It blocks every request
// outside the static server's own ORIGIN — a hostname-only check would let a
// regression open another loopback port (a daemon, some other local service)
// while the smoke stayed green — loads the reproducible artifact, and asserts
// the shared shell renders with the design system driving COMPUTED style,
// reproducing the #158 spike's "does the existing CSS survive the renderer
// swap" guard rather than trusting class presence. It runs offline and needs
// no daemon: #176 renders against the deterministic mock.
test.beforeEach(async ({ page, baseURL }) => {
  const origin = baseURL === undefined ? undefined : new URL(baseURL).origin;
  await page.route("**/*", (route) => {
    const url = new URL(route.request().url());
    if (origin !== undefined && url.origin === origin) {
      return route.continue();
    }
    return route.abort();
  });
});

test("the Dioxus shell renders offline with the shared design system", async ({ page }) => {
  await page.goto("/");

  // The wasm module mounts the root; wait for the shell it renders.
  const root = page.locator("#app-root");
  await expect(root).toBeVisible();
  await expect(page.locator("#sidebar")).toBeAttached();
  await expect(page.locator("#center")).toBeAttached();
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
// lifecycle fold broke, the status footer would stay stuck before "Ready" and
// the boot screen would never unmount.
test("the mock drives the shell to the Ready lifecycle state", async ({ page }) => {
  await page.goto("/");

  // After the mock settles, StatusFooter renders "client · Ready".
  // `toContainText` waits (default 5 s) so this handles the async WASM settle.
  const footer = page.locator("#status-footer");
  await expect(footer).toContainText("Ready");

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
  await expect(page.locator("#status-footer")).toContainText("Ready");
});
