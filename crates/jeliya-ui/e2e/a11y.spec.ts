import { test, expect } from "@playwright/test";

import {
  expectCleanNetwork,
  gotoReadyShell,
  installNoNetworkGuard,
  openDiagnostics,
  type NetworkGuard,
} from "./a11y-harness";

// The #177 structural accessibility contracts (§7): landmarks and headings,
// skip-link focus movement, dialog focus containment/Escape/return, the
// announce-once live region, compact target-size floors, forced reduced
// motion, and the document title. These are the keyboard/focus behaviours an
// axe scan cannot see; the axe sweep itself lives in a11y-matrix.spec.ts.
//
// Every test runs offline against the reproducible dist/ under the four
// viewport projects (wide/medium/compact/narrow) with reduced motion forced.

let guard: NetworkGuard;

test.beforeEach(async ({ page, baseURL }) => {
  guard = await installNoNetworkGuard(page, baseURL);
  // The announce-once witness must be installed BEFORE the app boots: it
  // records every distinct text the live region ever holds, so a region that
  // announced per-render (the coalescing failure mode) is caught even though
  // the final DOM looks identical.
  await page.addInitScript(() => {
    const log: string[] = [];
    (window as unknown as { __liveRegionLog: string[] }).__liveRegionLog = log;
    new MutationObserver(() => {
      const region = document.getElementById("live-region");
      if (region === null) {
        return;
      }
      const text = region.textContent ?? "";
      if (log.length === 0 || log[log.length - 1] !== text) {
        log.push(text);
      }
    // Observe the Document node, not `documentElement`: an init script runs
    // before the document has a root element, so observing the (always
    // present) Document is what makes the witness survive the boot.
    }).observe(document, {
      subtree: true,
      childList: true,
      characterData: true,
    });
  });
});

test.afterEach(() => {
  expectCleanNetwork(guard);
});

test("the settled shell has exactly one main landmark and one h1", async ({ page }, testInfo) => {
  await gotoReadyShell(page);

  // The boot cover carries its own main/h1 while it is the component root;
  // once Ready it must be UNMOUNTED (render.spec pins that), leaving exactly
  // one of each in the DOM: the landmarked center and its "Choose a room"
  // heading.
  await expect(page.locator("main")).toHaveCount(1);
  await expect(page.locator("h1")).toHaveCount(1);
  await expect(page.locator("#boot-screen")).not.toBeAttached();

  // The sidebar is a NAMED navigation landmark, distinguishable from main.
  const nav = page.locator("nav#rooms-nav");
  await expect(nav).toBeVisible();
  await expect(nav).toHaveAttribute("aria-label", /.+/);

  if (testInfo.project.name === "wide" || testInfo.project.name === "medium") {
    // Desktop layouts show both panes: exactly one VISIBLE main and h1.
    await expect(page.locator("main:visible")).toHaveCount(1);
    await expect(page.locator("h1:visible")).toHaveCount(1);
  } else {
    // Compact/narrow is the pane contract (render.spec pins it): the fixed
    // `pane-rooms` state shows ONLY the sidebar, so the main region (and its
    // h1) is present but hidden until pane navigation arrives with the Room
    // Workbench port.
    await expect(page.locator("main")).not.toBeVisible();
  }
});

test("skip links are the first tab stops and MOVE focus to their landmarks", async ({ page }, testInfo) => {
  await gotoReadyShell(page);

  // First Tab from a fresh document lands on the first skip link — nothing
  // may sit in the tab order before it.
  await page.keyboard.press("Tab");
  const first = await page.evaluate(() => {
    const active = document.activeElement;
    return { class: active?.className ?? "", href: active?.getAttribute("href") ?? "" };
  });
  expect(first.class).toContain("skip-link");
  expect(first.href).toBe("#rooms-nav");

  // Activating it moves FOCUS (not just scroll) into the rooms navigation
  // landmark: the nav carries tabindex="-1" exactly so it can receive it.
  await page.keyboard.press("Enter");
  await expect
    .poll(async () => page.evaluate(() => document.activeElement?.id ?? "<none>"))
    .toBe("rooms-nav");

  // The second skip link is the next tab stop and moves focus to main. Its
  // target is only focusable where it is visible — the desktop layouts; on
  // compact/narrow the fixed `pane-rooms` state hides the main region (see
  // the landmark test), so the browser refuses the focus move there.
  if (testInfo.project.name === "wide" || testInfo.project.name === "medium") {
    await gotoReadyShell(page);
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    const second = await page.evaluate(
      () => document.activeElement?.getAttribute("href") ?? "",
    );
    expect(second).toBe("#main-content");
    await page.keyboard.press("Enter");
    await expect
      .poll(async () => page.evaluate(() => document.activeElement?.id ?? "<none>"))
      .toBe("main-content");
  }
});

test("the Diagnostics dialog traps focus, closes on Escape, and returns focus to its opener", async ({ page }) => {
  await gotoReadyShell(page);
  await openDiagnostics(page);

  // Initial focus is the dialog PANEL (tabindex="-1"), never a control — so a
  // destructive control can never be the landing spot (§5.6). openDiagnostics
  // already asserted activeElement is #dialog-panel.

  // Containment: tabbing forward repeatedly must never land outside the
  // dialog subtree. The sentinels redirect asynchronously, so poll until
  // focus settles inside after each Tab.
  for (let i = 0; i < 6; i += 1) {
    await page.keyboard.press("Tab");
    await expect
      .poll(async () =>
        page.evaluate(() => {
          const active = document.activeElement;
          return active?.closest("#dialog-backdrop") === null ? "escaped" : "contained";
        }),
      )
      .toBe("contained");
  }
  // And backwards, past the leading edge.
  for (let i = 0; i < 3; i += 1) {
    await page.keyboard.press("Shift+Tab");
    await expect
      .poll(async () =>
        page.evaluate(() => {
          const active = document.activeElement;
          return active?.closest("#dialog-backdrop") === null ? "escaped" : "contained";
        }),
      )
      .toBe("contained");
  }

  // Escape closes the dialog — unmounted, not hidden — and focus returns to
  // the opener so the keyboard user is back where they left.
  await page.keyboard.press("Escape");
  await expect(page.locator("#dialog-backdrop")).not.toBeAttached();
  await expect
    .poll(async () => page.evaluate(() => document.activeElement?.id ?? "<none>"))
    .toBe("diagnostics-open");
});

test("the connection live region announces the settled room count exactly once", async ({ page }) => {
  await gotoReadyShell(page);

  // The region is one STABLE polite node.
  const region = page.locator("#live-region");
  await expect(region).toHaveAttribute("aria-live", "polite");
  await expect(region).toHaveAttribute("aria-atomic", "true");
  await expect(region).toHaveText("0 rooms");

  // The witness recorded every distinct text the region ever held. A
  // coalescing announcer yields exactly one transition: "" → "0 rooms". More
  // entries mean the announce-once contract broke (per-render re-announce);
  // the checklist's exact failure mode.
  const log = await page.evaluate(
    () => (window as unknown as { __liveRegionLog: string[] }).__liveRegionLog,
  );
  expect(log.filter((entry) => entry !== "")).toEqual(["0 rooms"]);
});

test("visible interactive targets meet the compact target-size floors", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name !== "compact" && testInfo.project.name !== "narrow",
    "target-size floors are the compact/narrow contract (§7)",
  );
  await gotoReadyShell(page);

  // Hit-test the real geometry of every visible interactive control (WCAG
  // 2.5.8): at least 24×24 always; a target under 44px in either dimension
  // must not have another interactive target's center within 24px of its own
  // (the spacing exception). Skip links are visually hidden until focused, so
  // only currently-visible controls are measured — measuring rendered
  // geometry, not CSS declarations.
  const targets = page.locator("button:visible, a:visible, [role='button']:visible");
  const boxes = [];
  for (const target of await targets.all()) {
    const box = await target.boundingBox();
    // The 1px-clip visually-hidden pattern (skip links until focused) is not
    // a pointer target — it expands to full size exactly when focused, which
    // the skip-link test exercises — so it is excluded from hit-testing.
    if (box !== null && box.width > 2 && box.height > 2) {
      boxes.push(box);
    }
  }
  expect(boxes.length, "the settled shell must expose at least one interactive control").toBeGreaterThan(0);
  for (const box of boxes) {
    expect(box.width, `target width ${box.width} under the 24px floor`).toBeGreaterThanOrEqual(24);
    expect(box.height, `target height ${box.height} under the 24px floor`).toBeGreaterThanOrEqual(24);
  }
  for (let i = 0; i < boxes.length; i += 1) {
    const a = boxes[i];
    if (a.width >= 44 && a.height >= 44) {
      continue;
    }
    for (let j = 0; j < boxes.length; j += 1) {
      if (i === j) {
        continue;
      }
      const b = boxes[j];
      const dx = a.x + a.width / 2 - (b.x + b.width / 2);
      const dy = a.y + a.height / 2 - (b.y + b.height / 2);
      const distance = Math.sqrt(dx * dx + dy * dy);
      expect(
        distance,
        `sub-44px target needs 24px of breathing room from its neighbor (got ${distance.toFixed(1)}px)`,
      ).toBeGreaterThanOrEqual(24);
    }
  }
});

test("reduced motion is forced for the whole a11y matrix", async ({ page }) => {
  await gotoReadyShell(page);

  // Every a11y project runs with reducedMotion: 'reduce' (§7). The foundation
  // shell carries no motion-bearing element yet — the three animated elements
  // in styles.css are React chat surfaces — so the honest assertion is that
  // the context every foundation element renders under IS reduced motion;
  // the branch-difference proof starts existing when a foundation element
  // carries motion.
  const reduced = await page.evaluate(
    () => window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  );
  expect(reduced).toBe(true);
});

test("the document title names the destination", async ({ page }) => {
  await gotoReadyShell(page);

  // The foundation is a single route, so the destination is the app itself:
  // the (never-translated) brand. Route-specific titles arrive with the
  // Room Workbench port.
  await expect(page).toHaveTitle("Jeliya");
});
