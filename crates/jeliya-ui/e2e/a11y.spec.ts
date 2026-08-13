import { test, expect, type Page, type Locator } from "@playwright/test";

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
  // The announce-once witness must be installed BEFORE the app boots. It records
  // both TEXT changes and NODE (re)mounts of the live region: a region that
  // re-announced per render, OR was remounted (a new node) carrying the same
  // message, is an announce-once regression assistive tech can miss — and a
  // dedup on distinct-consecutive-text alone would hide the remount.
  await page.addInitScript(() => {
    const log: { type: string; text: string }[] = [];
    (window as unknown as { __liveRegionLog: typeof log }).__liveRegionLog = log;
    let lastNode: Element | null = null;
    let lastText: string | null = null;
    new MutationObserver(() => {
      const region = document.getElementById("live-region");
      if (region === null) {
        return;
      }
      const text = region.textContent ?? "";
      if (region !== lastNode) {
        // A (re)mount of the region node — recorded even if the text is
        // unchanged, so a stable-node regression is visible.
        log.push({ type: "mount", text });
        lastNode = region;
        lastText = text;
        return;
      }
      if (text !== lastText) {
        log.push({ type: "text", text });
        lastText = text;
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

test("the settled shell has exactly one main landmark and one h1 on every viewport", async ({ page }, testInfo) => {
  await gotoReadyShell(page);

  // Exactly one <main> and one <h1> in the DOM, and the main landmark is
  // VISIBLE on EVERY viewport — the rooms pane is the main, and `pane-rooms`
  // keeps it shown on compact too, so no viewport is left without a main in the
  // accessibility tree. The single h1 lives at the always-rendered root
  // (visually hidden; the visible headings are the room-list nav's name and the
  // centre's h2).
  await expect(page.locator("main")).toHaveCount(1);
  await expect(page.locator("main:visible")).toHaveCount(1);
  await expect(page.locator("h1")).toHaveCount(1);
  await expect(page.locator("#boot-screen")).not.toBeAttached();

  // The room list is a NAMED navigation landmark inside main.
  const nav = page.locator("nav#rooms-nav");
  await expect(nav).toBeVisible();
  await expect(nav).toHaveAttribute("aria-label", /.+/);

  if (testInfo.project.name === "wide" || testInfo.project.name === "medium") {
    // Desktop shows the detail pane too, carrying the visible h2 under the h1.
    await expect(page.locator("#center h2:visible")).toHaveCount(1);
  } else {
    // Compact/narrow is the pane contract (render.spec pins it): `pane-rooms`
    // shows only the rooms (main) pane and hides the `.center` detail section.
    await expect(page.locator("#center")).not.toBeVisible();
  }
});

test("the skip link is the first tab stop and MOVES focus to the rooms landmark", async ({ page }) => {
  await gotoReadyShell(page);

  // First Tab from a fresh document lands on the (only) skip link — nothing may
  // sit in the tab order before it. The foundation offers one link, "skip to
  // rooms": the rooms list is the one meaningful content region and is visible
  // on every viewport, so the link is never broken (a "skip to content" link
  // pointing at the compact-hidden `.center` is deliberately not offered).
  await page.keyboard.press("Tab");
  const first = await page.evaluate(() => {
    const active = document.activeElement;
    return { class: active?.className ?? "", href: active?.getAttribute("href") ?? "" };
  });
  expect(first.class).toContain("skip-link");
  expect(first.href).toBe("#rooms-nav");

  // The FOCUSED skip link must become VISIBLE with usable geometry — the
  // `:focus-visible` expansion of the hidden-until-focused (1×1 clip) control. Focus
  // ORDER alone (above) still passes if that expansion rule regresses, but a keyboard
  // user could not SEE the focused control, so measure its rendered box now that it
  // holds focus: it must be far larger than the 1px clip and a usable target.
  const focusedBox = await page.locator(".skip-link:focus").boundingBox();
  expect(focusedBox, "the focused skip link must have a rendered box").not.toBeNull();
  expect(
    focusedBox!.width,
    `focused skip link width ${focusedBox!.width} must expand past the 1px clip to a usable target`,
  ).toBeGreaterThanOrEqual(24);
  expect(
    focusedBox!.height,
    `focused skip link height ${focusedBox!.height} must expand past the 1px clip`,
  ).toBeGreaterThanOrEqual(16);

  // Activating it moves FOCUS (not just scroll) into the rooms navigation
  // landmark: the nav carries tabindex="-1" exactly so it can receive it.
  await page.keyboard.press("Enter");
  await expect
    .poll(async () => page.evaluate(() => document.activeElement?.id ?? "<none>"))
    .toBe("rooms-nav");

  // There is exactly one skip link — the next Tab must leave the skip-links
  // container (no orphaned "skip to content" pointing at a hidden target).
  await expect(page.locator(".skip-link")).toHaveCount(1);
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

  // The witness recorded TEXT changes and NODE (re)mounts. A coalescing announcer
  // on ONE stable node yields: the region mounted exactly once, and "0 rooms"
  // reached exactly once — a remount (mount count > 1) or a re-announce ("0 rooms"
  // more than once) is the checklist's exact failure mode.
  const log = await page.evaluate(
    () =>
      (window as unknown as { __liveRegionLog: { type: string; text: string }[] })
        .__liveRegionLog,
  );
  expect(
    log.filter((entry) => entry.type === "mount").length,
    "the live region must be mounted once, never remounted",
  ).toBe(1);
  expect(
    log.filter((entry) => entry.text === "0 rooms").length,
    "the settled room count must be announced exactly once",
  ).toBe(1);
});

// Hit-test the real geometry of every visible interactive control (WCAG 2.5.8):
// at least 24×24 always; a target under 44px in either dimension must keep a
// >=24px GAP from its neighbor's boundary on at least one axis (the spacing
// exception, measured boundary-to-boundary as the retiring check does — not
// center distance). Skip links are visually hidden until focused, so only
// currently-visible controls are measured — rendered geometry, not CSS. NATIVE
// form controls (`input`/`textarea`/`select`, e.g. inside the Field primitive)
// are interactive targets too, so they are measured alongside buttons and links —
// a compact Field input that regresses below the 24px floor must be caught.
// A control MAY use the documented compact exception (24–43px + a >=24px spacing
// gap) ONLY if it is listed here with a reason. Empty today: every foundation
// control meets the 44px compact floor, so a newly undersized isolated control is
// caught rather than auto-excused by incidental surrounding space.
const COMPACT_44_EXCEPTIONS: { selector: string; reason: string }[] = [];

async function assertTargetGeometry(
  page: Page,
  where: string,
  root: Page | Locator = page,
): Promise<void> {
  // Scope to `root` so a modal measurement covers only the dialog's own controls,
  // not the shell controls BEHIND the backdrop — those are legitimately covered by
  // the modal and would otherwise fail the hit-test.
  const targets = root.locator(
    "button:visible, a:visible, input:visible, textarea:visible, select:visible, [role='button']:visible",
  );
  const measured: { box: { x: number; y: number; width: number; height: number }; isException: boolean }[] = [];
  for (const target of await targets.all()) {
    const box = await target.boundingBox();
    // The 1px-clip visually-hidden pattern (skip links until focused) is not a
    // pointer target — it expands to full size exactly when focused — so it is
    // excluded from hit-testing.
    if (box === null || box.width <= 2 || box.height <= 2) {
      continue;
    }
    // HIT-TEST, do not merely trust the bounding box: an overlay covering the
    // control still lets `boundingBox()` report its full size, so a 44×44 box could
    // deliver taps to an overlay. `elementFromPoint` at the control's centre must
    // resolve to the control or a descendant, or the target is not actually reachable.
    const reachable = await target.evaluate((el) => {
      const r = el.getBoundingClientRect();
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      // Reachable if the centre resolves to the control, a DESCENDANT (a
      // pseudo-element hit reports its originating element), or an ANCESTOR (the
      // control's centre sits on padding/background that hit-tests to a container it
      // is inside) — matching the retiring check. A FOREIGN element means an overlay.
      return (
        hit !== null &&
        (el === hit || (hit instanceof Node && el.contains(hit)) || (hit instanceof Node && hit.contains(el)))
      );
    });
    expect(reachable, `${where}: a target is covered — a tap at its centre does not reach it`).toBe(true);
    let isException = false;
    for (const ex of COMPACT_44_EXCEPTIONS) {
      if (await target.evaluate((el, sel) => el.matches(sel), ex.selector)) {
        isException = true;
        break;
      }
    }
    measured.push({ box, isException });
  }
  expect(measured.length, `${where}: at least one interactive control`).toBeGreaterThan(0);
  for (const { box } of measured) {
    expect(box.width, `${where}: target width ${box.width} under the 24px floor`).toBeGreaterThanOrEqual(24);
    expect(box.height, `${where}: target height ${box.height} under the 24px floor`).toBeGreaterThanOrEqual(24);
  }
  for (let i = 0; i < measured.length; i += 1) {
    const { box: a, isException } = measured[i];
    if (a.width >= 44 && a.height >= 44) {
      continue;
    }
    // A sub-44px target is allowed ONLY as a DOCUMENTED exception — the compact
    // floor is 44px by default, so an undersized isolated control cannot regress in
    // under incidental spacing.
    expect(
      isException,
      `${where}: a ${a.width}×${a.height} target is below the 44px compact floor and is not a documented exception (add it to COMPACT_44_EXCEPTIONS with a reason, or enlarge it)`,
    ).toBe(true);
    for (let j = 0; j < measured.length; j += 1) {
      if (i === j) {
        continue;
      }
      const b = measured[j].box;
      // The GAP between the rectangle BOUNDARIES on whichever axis separates them,
      // NOT center distance: two 24px controls touching edge-to-edge have centers
      // 24px apart yet ZERO breathing room. Overlap on an axis contributes a
      // negative gap, so two boxes must be clear by >=24px on at least ONE axis.
      const vertical = Math.max(a.y - (b.y + b.height), b.y - (a.y + a.height));
      const horizontal = Math.max(a.x - (b.x + b.width), b.x - (a.x + a.width));
      const gap = Math.max(vertical, horizontal);
      expect(
        gap,
        `${where}: sub-44px exception needs a 24px gap from its neighbor's boundary (got ${gap.toFixed(1)}px)`,
      ).toBeGreaterThanOrEqual(24);
    }
  }
}

test("visible interactive targets meet the compact target-size floors", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name !== "compact" && testInfo.project.name !== "narrow",
    "target-size floors are the compact/narrow contract (§7)",
  );
  await gotoReadyShell(page);
  await assertTargetGeometry(page, "settled shell");
  // The Diagnostics dialog's controls must meet the SAME floors: the settled-shell
  // sweep cannot see them (dialog closed), and the dialog axe sweep checks roles,
  // not geometry — so a dialog control could regress below 24px or crowd a neighbor
  // while this required context stays green.
  await openDiagnostics(page);
  await assertTargetGeometry(page, "diagnostics dialog", page.locator("#dialog-backdrop"));
});

test("reduced motion disables the reconnecting-status animation", async ({ page }) => {
  await gotoReadyShell(page);

  // Every a11y project runs with reducedMotion: 'reduce' (§7).
  expect(
    await page.evaluate(() => window.matchMedia("(prefers-reduced-motion: reduce)").matches),
  ).toBe(true);

  // The foundation renders a PULSING dot for the reconnecting (Interrupted) status
  // (`.conn-badge.conn-reconnecting .dot { animation: pulse … }`, with a
  // reduced-motion `animation: none` override). The settled shell shows Ready, and
  // there is no e2e hook to drive Interrupted, so mount the EXACT canonical markup
  // to exercise the real CSS rule — then compare the COMPUTED animation under both
  // media states. Asserting only the emulated media query (above) would still pass
  // if the reduced-motion override regressed.
  await page.evaluate(() => {
    const badge = document.createElement("div");
    badge.className = "conn-badge conn-reconnecting";
    badge.id = "e2e-motion-probe";
    const dot = document.createElement("span");
    dot.className = "dot";
    badge.appendChild(dot);
    document.body.appendChild(badge);
  });
  const dot = page.locator("#e2e-motion-probe .dot");

  await page.emulateMedia({ reducedMotion: "reduce" });
  expect(
    await dot.evaluate((el) => getComputedStyle(el).animationName),
    "the reconnecting dot must NOT animate under reduced motion",
  ).toBe("none");

  await page.emulateMedia({ reducedMotion: "no-preference" });
  expect(
    await dot.evaluate((el) => getComputedStyle(el).animationName),
    "the reconnecting dot DOES pulse when motion is allowed (so the override above is real)",
  ).toBe("pulse");

  // Restore the matrix-wide reduced-motion default for any later assertions.
  await page.emulateMedia({ reducedMotion: "reduce" });
});

test("the document title names the destination", async ({ page }) => {
  await gotoReadyShell(page);

  // The foundation is a single route, so the destination is the app itself:
  // the (never-translated) brand. Route-specific titles arrive with the
  // Room Workbench port.
  await expect(page).toHaveTitle("Jeliya");
});

test.describe("French browser locale", () => {
  // Playwright's `locale` sets navigator.language, which the web composition
  // reads (web-sys) and injects as the platform locale. This proves the whole
  // chain end to end: browser language -> platform_locale -> LocaleState::resolve
  // -> French catalog (#1) AND <html lang> (#3), with NO stored preference.
  test.use({ locale: "fr-FR" });

  test("navigator.language drives the French catalog and the document lang", async ({ page }) => {
    await gotoReadyShell(page);

    // #3: <html lang> tracks the resolved text locale (was permanently "en").
    await expect
      .poll(() => page.evaluate(() => document.documentElement.lang))
      .toBe("fr");

    // #1: a fresh fr-FR browser with no stored preference reaches the French
    // catalog — `Aucun salon` is the sidebar empty state, visible on every
    // viewport (the center is pane-hidden on compact).
    await expect(page.locator("#rooms-empty")).toContainText("Aucun salon");
  });
});
