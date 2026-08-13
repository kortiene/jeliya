import { expect, type Page, type TestInfo } from "@playwright/test";

// The shared offline harness for the #177 accessibility specs. Mirrors the
// render smoke's no-network contract (render.spec.ts): every request outside
// the static server's own origin is blocked, same-origin API-shaped traffic
// (/api/*, /ws) is recorded and aborted as a mock-boundary leak, and every
// WebSocket attempt is blocked and recorded. The a11y specs run against the
// same reproducible dist/ the render smoke serves — offline, no daemon.

export interface NetworkGuard {
  webSockets: string[];
  apiRequests: string[];
  offOrigin: string[];
}

export async function installNoNetworkGuard(
  page: Page,
  baseURL: string | undefined,
): Promise<NetworkGuard> {
  // Reduced motion is forced here as well as in the project `use` blocks:
  // @playwright/test 1.61.1 does not propagate the `reducedMotion` use option
  // into the browser context (verified empirically — `matchMedia` stays
  // `no-preference` under both project- and spec-level `use`), while the CDP
  // emulation below is honored. The a11y specs assert `matchMedia` so a
  // future Playwright bump that changes this stays visible.
  await page.emulateMedia({ reducedMotion: "reduce" });
  const guard: NetworkGuard = { webSockets: [], apiRequests: [], offOrigin: [] };
  const origin = baseURL === undefined ? undefined : new URL(baseURL).origin;
  await page.route("**/*", (route) => {
    const url = new URL(route.request().url());
    if (origin !== undefined && url.origin === origin) {
      const apiShaped =
        url.pathname.startsWith("/api/") ||
        url.pathname === "/ws" ||
        url.pathname.startsWith("/ws/");
      if (apiShaped) {
        guard.apiRequests.push(url.pathname);
        return route.abort();
      }
      return route.continue();
    }
    // An OFF-ORIGIN request (telemetry, a remote font, a CDN asset). Its failure
    // may not break rendering, so aborting silently would let the suite stay
    // green while the artifact is no longer network-silent. RECORD it so
    // `expectCleanNetwork` fails on any external attempt.
    guard.offOrigin.push(url.href);
    return route.abort();
  });
  await page.routeWebSocket(/.*/, () => {
    // Never connect: the socket stays dead.
  });
  page.on("websocket", (ws) => {
    guard.webSockets.push(ws.url());
  });
  return guard;
}

export function expectCleanNetwork(guard: NetworkGuard): void {
  expect(
    guard.webSockets,
    "the offline a11y suite must see no WebSocket connections (no-network guarantee)",
  ).toEqual([]);
  expect(
    guard.apiRequests,
    "the offline a11y suite must see no API-shaped same-origin traffic (mock boundary)",
  ).toEqual([]);
  expect(
    guard.offOrigin,
    "the offline a11y suite must attempt no off-origin requests (network-silent artifact)",
  ).toEqual([]);
}

// Load the shell and wait for the SETTLED ready state: the mock has driven the
// client to Ready and the scripted empty room.list read has answered
// (#rooms-empty visible, #rooms-loading gone) — the same settle contract the
// render smoke pins. Structural assertions made before settle would race the
// wasm boot.
export async function gotoReadyShell(page: Page): Promise<void> {
  await page.goto("/");
  // Structural settle witnesses, not copy: the boot cover must be UNMOUNTED
  // (the lifecycle reached Ready) and the answered empty state visible with
  // the loading placeholder gone (the scripted read settled). Catalog copy is
  // asserted only where copy IS the contract.
  await expect(page.locator("#rooms-empty")).toBeVisible();
  await expect(page.locator("#boot-screen")).not.toBeAttached();
  await expect(page.locator("#rooms-loading")).not.toBeAttached();
}

// Load the Ready shell with a POPULATED room list (`?rooms=N`, honored by
// `web_composition`), so the target-size sweep measures real `.room-select` rows
// — the empty shell renders none. Marker-gated exactly like `gotoBootCover`, so
// the fixture never activates in production.
export async function gotoReadyShellWithRooms(page: Page, count: number): Promise<void> {
  await page.addInitScript(() => {
    window.localStorage.setItem("jeliya-e2e-boot-fixture", "1");
  });
  await page.goto(`/?rooms=${count}`);
  await expect(page.locator("#boot-screen")).not.toBeAttached();
  await expect(page.locator("#rooms-loading")).not.toBeAttached();
  await expect(page.locator(".room-select").first()).toBeVisible();
}

// Load the app into the deterministic BOOT/terminal cover fixture
// (`?boot=<state>`, honored by WebRoot) and wait until the cover is mounted and
// the shell is NOT — so the boot branch, unreachable once the mock settles, can
// be axe-swept.
export async function gotoBootCover(page: Page, state: string): Promise<void> {
  // The fixture activates ONLY on this explicit test marker — never the hostname
  // (the packaged daemon serves the production UI from 127.0.0.1 too, so a
  // hostname gate would leave `?boot=` live in the real app). Set it before any
  // app script runs; the marker string is shared verbatim with
  // `BOOT_FIXTURE_MARKER` in `src/compose.rs`.
  await page.addInitScript(() => {
    window.localStorage.setItem("jeliya-e2e-boot-fixture", "1");
  });
  await page.goto(`/?boot=${state}`);
  await expect(page.locator("#boot-screen")).toBeVisible();
  await expect(page.locator("#app-root")).not.toBeAttached();
}

// Open the Diagnostics dialog from the status footer and wait until its panel
// holds focus (initial focus is asynchronous: the panel focuses itself from
// its onmounted handler).
export async function openDiagnostics(page: Page): Promise<void> {
  await page.locator("#diagnostics-open").click();
  await expect(page.locator("#dialog-panel")).toBeVisible();
  await expect
    .poll(async () =>
      page.evaluate(() => document.activeElement?.id ?? "<none>"),
    )
    .toBe("dialog-panel");
}

// Attach a JSON body to the report so advisory findings stay auditable
// without failing the run.
export async function attachJson(
  testInfo: TestInfo,
  name: string,
  body: unknown,
): Promise<void> {
  await testInfo.attach(name, {
    body: JSON.stringify(body, null, 2),
    contentType: "application/json",
  });
}
