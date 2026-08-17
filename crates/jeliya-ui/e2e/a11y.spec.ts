import { test, expect, type Page, type Locator } from "@playwright/test";

import {
  expectCleanNetwork,
  gotoReadyShell,
  gotoReadyShellWithRooms,
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
    // Record from the mutation RECORDS, not by re-reading `textContent` at callback
    // time: the observer batches records into one microtask, so a node inserted empty
    // and then updated in the same batch would read its FINAL text at the mount record.
    // Deriving each entry from the record itself keeps mount-vs-update distinguishable
    // regardless of batching — a `mount` for the region node's insertion, and a `text`
    // for every later text change (a `characterData` edit OR a replacement text node
    // added under the region). A remount (mount>1) or a re-announce (duplicate text) is
    // the announce-once failure mode; content present at INSERTION (a mount but no `text`
    // update) is the "not reliably announced" failure the checklist warns about.
    const inRegion = (n: Node | null): boolean =>
      !!n && !!(n as Element).closest && !!(n as Element).closest("#live-region");
    new MutationObserver((records) => {
      // A `characterData` record's NEW value is NOT `target.data` at callback
      // time: the observer batches records into one microtask, so a node edited
      // twice in one batch reads its FINAL text for both records. With
      // `characterDataOldValue`, each record's new value is the NEXT same-target
      // record's `oldValue` in this batch — or `target.data` for the last one.
      const newValueOf = (i: number): string => {
        const rec = records[i];
        for (let j = i + 1; j < records.length; j++) {
          const later = records[j];
          if (later.type === "characterData" && later.target === rec.target) {
            return later.oldValue ?? "";
          }
        }
        return (rec.target as Text).data ?? "";
      };
      for (let i = 0; i < records.length; i++) {
        const rec = records[i];
        for (const n of Array.from(rec.addedNodes)) {
          if (n.nodeType === 1) {
            const el = n as Element;
            const region = el.id === "live-region" ? el : el.querySelector?.("#live-region");
            if (region) log.push({ type: "mount", text: region.textContent ?? "" });
          } else if (
            n.nodeType === 3 &&
            (rec.target as Element)?.id === "live-region"
          ) {
            // A replacement text node added under the region = a text update.
            log.push({ type: "text", text: (n as Text).data ?? "" });
          }
        }
        if (rec.type === "characterData" && inRegion((rec.target as Node).parentNode)) {
          log.push({ type: "text", text: newValueOf(i) });
        }
      }
    // Observe the Document node, not `documentElement`: an init script runs
    // before the document has a root element, so observing the (always
    // present) Document is what makes the witness survive the boot.
    }).observe(document, {
      subtree: true,
      childList: true,
      characterData: true,
      characterDataOldValue: true,
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

  // The close (×) control is explicitly type="button": this shared Dialog may be
  // rendered inside a form, where the default type="submit" would make dismissing
  // the dialog also submit the enclosing form (#274 25-8).
  await expect(page.locator("#dialog-panel .modal-head .icon-btn")).toHaveAttribute(
    "type",
    "button",
  );

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

test("a terminal cover that replaces an open dialog takes focus, not the body", async ({ page }) => {
  // Arm the marker-gated connection hook (no `?boot=`, so the shell still reaches Ready).
  await page.addInitScript(() => {
    window.localStorage.setItem("jeliya-e2e-boot-fixture", "1");
  });
  await gotoReadyShell(page);
  await openDiagnostics(page);
  await expect(page.locator("#dialog-backdrop")).toBeAttached();
  // Drive a TERMINAL transition (Ready → Failed) while Diagnostics is open: the shell
  // and the dialog's opener unmount together, so the opener's focus-restoration cannot
  // run. The cover must take focus, not let it fall to the document body.
  await page.waitForFunction(
    () =>
      typeof (window as unknown as { __jeliyaE2eConnState?: unknown }).__jeliyaE2eConnState ===
      "function",
  );
  await page.evaluate(() =>
    (window as unknown as { __jeliyaE2eConnState: (s: string) => void }).__jeliyaE2eConnState(
      "failed",
    ),
  );
  await expect(page.locator("#boot-screen")).toBeVisible();
  await expect(page.locator("#dialog-backdrop")).not.toBeAttached();
  await expect
    .poll(async () => page.evaluate(() => document.activeElement?.id ?? "<none>"))
    .toBe("boot-screen");
});

test("the room-count live region announces the settled count exactly once", async ({ page }) => {
  await gotoReadyShell(page);

  // The CONTENT region is one STABLE polite node carrying the room count.
  const region = page.locator("#live-region");
  await expect(region).toHaveAttribute("aria-live", "polite");
  await expect(region).toHaveAttribute("aria-atomic", "true");
  await expect(region).toHaveText("0 rooms");

  // The witness recorded TEXT changes and NODE (re)mounts. A coalescing announcer on
  // ONE stable node yields: the region mounted exactly once and EMPTY, and "0 rooms"
  // reached exactly once as a subsequent TEXT UPDATE. Polite live-region content that
  // is already present when the node is INSERTED is not reliably announced by assistive
  // tech, so requiring the mount to be empty and the count to arrive as an update is
  // what actually proves the stable pre-existing region receives the announcement — a
  // remount, a re-announce, or content-at-insertion is the checklist's failure mode.
  const log = await page.evaluate(
    () =>
      (window as unknown as { __liveRegionLog: { type: string; text: string }[] })
        .__liveRegionLog,
  );
  const mounts = log.filter((entry) => entry.type === "mount");
  expect(mounts.length, "the live region must be mounted once, never remounted").toBe(1);
  // The count must arrive as a TEXT UPDATE (a `characterData` edit or a replacement
  // text node), proving the stable pre-existing region RECEIVED an announcement — not
  // as content present at the node's insertion (which the mount entry would carry and
  // which is not reliably announced), and not re-announced (that would be >1). This is
  // derived from the mutation RECORDS, so observer batching cannot disguise a mount as
  // an update or vice versa.
  expect(
    log.filter((entry) => entry.type === "text" && entry.text === "0 rooms").length,
    "the settled room count must arrive as exactly one TEXT update, not content-at-insertion",
  ).toBe(1);
});

test("the connection live region announces a drop and its recovery exactly once each", async ({
  page,
}) => {
  // Arm the e2e marker (before any app script) so the marker-gated connection hook
  // installs — the same `localStorage` marker the `?boot=` fixture uses; with no
  // `?boot=` query the boot fixture stays inactive, so the shell still reaches Ready.
  await page.addInitScript(() => {
    window.localStorage.setItem("jeliya-e2e-boot-fixture", "1");
  });
  // Witness the DEDICATED connection region's text changes and remounts, installed
  // before boot on the always-present Document (the region mounts during boot).
  await page.addInitScript(() => {
    const log: { type: string; text: string }[] = [];
    (window as unknown as { __connLog: typeof log }).__connLog = log;
    let lastNode: Element | null = null;
    let lastText: string | null = null;
    new MutationObserver(() => {
      const region = document.getElementById("connection-live-region");
      if (region === null) {
        return;
      }
      const text = region.textContent ?? "";
      if (region !== lastNode) {
        log.push({ type: "mount", text });
        lastNode = region;
        lastText = text;
        return;
      }
      if (text !== lastText) {
        log.push({ type: "text", text });
        lastText = text;
      }
    }).observe(document, { subtree: true, childList: true, characterData: true });
  });
  await gotoReadyShell(page);

  const region = page.locator("#connection-live-region");
  await expect(region).toHaveAttribute("aria-live", "polite");
  // A settled shell (Ready, no prior drop) has announced NOTHING here: the happy boot
  // path (Connecting → Ready) is deliberately silent — only a drop or a recovery speaks.
  await expect(region).toHaveText("");

  // The announcer reacts to each StateChanged EVENT (not a render snapshot), so drive a
  // REAL drop then recovery through the marker-gated e2e hook and WAIT for each to render
  // — stepping them so the pair is two observed transitions, not a batched net-Ready.
  await page.waitForFunction(
    () =>
      typeof (window as unknown as { __jeliyaE2eConnState?: unknown }).__jeliyaE2eConnState ===
      "function",
  );
  await page.evaluate(() =>
    (
      window as unknown as { __jeliyaE2eConnState: (s: string) => void }
    ).__jeliyaE2eConnState("interrupted"),
  );
  await expect(region).not.toHaveText(""); // the drop announcement rendered
  const dropText = (await region.textContent()) ?? "";
  await page.evaluate(() =>
    (window as unknown as { __jeliyaE2eConnState: (s: string) => void }).__jeliyaE2eConnState(
      "ready",
    ),
  );
  // The recovery REPLACES the drop text (a distinct, non-empty announcement).
  await expect.poll(async () => (await region.textContent()) ?? "").not.toBe(dropText);

  // The region stayed one node and received EXACTLY two non-empty announcements — the
  // drop and its recovery, each once. Removing #connection-live-region or breaking the
  // Interrupted↔Ready wiring makes this fail (0 announcements); a re-announce makes it >2.
  const log = await page.evaluate(
    () => (window as unknown as { __connLog: { type: string; text: string }[] }).__connLog,
  );
  expect(
    log.filter((e) => e.type === "mount").length,
    `the connection region must be a stable node: ${JSON.stringify(log)}`,
  ).toBe(1);
  const announcements = log.filter((e) => e.type === "text" && e.text.trim() !== "");
  expect(
    announcements.length,
    `the drop and recovery must each be announced exactly once: ${JSON.stringify(log)}`,
  ).toBe(2);
});

// #276 — the complementary proof: both the DROP and the RECOVERY are heard even
// when both StateChanged events are already buffered before Dioxus renders (the
// batched scenario). The existing stepped test above sidesteps this by waiting for
// the drop text to render before driving `ready`; this test removes that crutch.
test("rapid batched drop→recovery announces both states without a render wait between", async ({
  page,
}) => {
  // Arm the marker-gated connection hook (no `?boot=`, so the shell still reaches Ready).
  await page.addInitScript(() => {
    window.localStorage.setItem("jeliya-e2e-boot-fixture", "1");
  });
  // Install a RECORD-based witness for #connection-live-region before the app boots.
  // Records are read from the mutation RECORDS directly (characterData edits and
  // replacement text nodes), NOT by re-reading textContent at observer-callback time:
  // the observer batches records into one microtask, so re-reading textContent would
  // see only the final DOM text of that batch and silently miss the intermediate drop —
  // exactly the failure this test must detect (#276 AC-1). Reading from the record
  // itself keeps each mutation distinct regardless of how the browser batches callbacks.
  await page.addInitScript(() => {
    const log: { type: string; text: string }[] = [];
    (window as unknown as { __connRecordLog: typeof log }).__connRecordLog = log;
    const inConnRegion = (n: Node | null): boolean =>
      !!n && !!(n as Element).closest && !!(n as Element).closest("#connection-live-region");
    new MutationObserver((records) => {
      // Same batching rule as the shared witness: a `characterData` record's new
      // value is the NEXT same-target record's `oldValue` in this batch, or
      // `target.data` only for the last one — never `target.data` for all.
      const newValueOf = (i: number): string => {
        const rec = records[i];
        for (let j = i + 1; j < records.length; j++) {
          const later = records[j];
          if (later.type === "characterData" && later.target === rec.target) {
            return later.oldValue ?? "";
          }
        }
        return (rec.target as Text).data ?? "";
      };
      for (let i = 0; i < records.length; i++) {
        const rec = records[i];
        for (const n of Array.from(rec.addedNodes)) {
          if (n.nodeType === 1 /* ELEMENT_NODE */) {
            const el = n as Element;
            const region =
              el.id === "connection-live-region"
                ? el
                : el.querySelector?.("#connection-live-region");
            if (region) {
              log.push({ type: "mount", text: region.textContent ?? "" });
            }
          } else if (
            n.nodeType === 3 /* TEXT_NODE */ &&
            (rec.target as Element)?.id === "connection-live-region"
          ) {
            // A replacement text node added directly under the region = a text update.
            log.push({ type: "text", text: (n as Text).data ?? "" });
          }
        }
        if (rec.type === "characterData" && inConnRegion((rec.target as Node).parentNode)) {
          log.push({ type: "text", text: newValueOf(i) });
        }
      }
    }).observe(document, {
      subtree: true,
      childList: true,
      characterData: true,
      characterDataOldValue: true,
    });
  });

  await gotoReadyShell(page);

  // Wait for the marker-gated e2e hook (installed once wasm boots and sees the marker).
  await page.waitForFunction(
    () =>
      typeof (window as unknown as { __jeliyaE2eConnState?: unknown }).__jeliyaE2eConnState ===
      "function",
  );

  // Drive BOTH transitions in ONE synchronous evaluate — no render wait between.
  // Both StateChanged events land in the subscriber buffer before the consume loop
  // yields, so the old single-signal announcer would overwrite the drop with the
  // recovery and only one announcement would be heard. The queue-based announcer
  // (#276) enqueues both as distinct messages and drains one per render frame, so
  // both states each occupy the live region for their own committed render.
  await page.evaluate(() => {
    const hook = (
      window as unknown as { __jeliyaE2eConnState: (s: string) => void }
    ).__jeliyaE2eConnState;
    hook("interrupted");
    hook("ready");
  });

  // Poll until both render frames have committed: the drain effect runs at lowest
  // priority and each drain step writes `displayed`, which forces a render before
  // the effect can run again — so two queued messages require two render cycles.
  type LogEntry = { type: string; text: string };
  await expect
    .poll(async () => {
      const entries: LogEntry[] = await page.evaluate(
        () =>
          (window as unknown as { __connRecordLog: { type: string; text: string }[] })
            .__connRecordLog,
      );
      return entries.filter((e) => e.type === "text" && e.text.trim() !== "").length;
    })
    .toBe(2);

  const log: LogEntry[] = await page.evaluate(
    () =>
      (window as unknown as { __connRecordLog: { type: string; text: string }[] })
        .__connRecordLog,
  );

  // The region node must be stable — mounted once and never remounted between the
  // two render frames that commit the drop and the recovery respectively.
  expect(
    log.filter((e) => e.type === "mount").length,
    `the connection region must mount once and stay stable across both render frames: ${JSON.stringify(log)}`,
  ).toBe(1);

  const announcements = log.filter((e) => e.type === "text" && e.text.trim() !== "");
  expect(
    announcements.length,
    `the drop and recovery must each be announced exactly once (not coalesced, not re-announced): ${JSON.stringify(log)}`,
  ).toBe(2);

  // The DROP (Interrupted → "Connection status: Reconnecting") must render BEFORE
  // the RECOVERY (Ready → "Connection status: Connected").
  expect(
    announcements[0].text,
    `first announcement must be the drop (Reconnecting): ${JSON.stringify(announcements)}`,
  ).toContain("Reconnecting");
  expect(
    announcements[1].text,
    `second announcement must be the recovery (Connected): ${JSON.stringify(announcements)}`,
  ).toContain("Connected");
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
    if (box === null) {
      continue; // not rendered at all
    }
    // The 1px-clip visually-hidden pattern (skip links until focused) is not a
    // pointer target — it expands to full size exactly when focused. Exempt ONLY that
    // known `.skip-link` pattern at this size: any OTHER visible interactive control
    // collapsed to 1–2px is a real regression (a CSS break), so let it fall through to
    // the hit-test and the 24px floor below, which fail it — rather than silently
    // skipping it and leaving the accessibility matrix green for an unusable control.
    if (box.width <= 2 || box.height <= 2) {
      const isHiddenSkipLink = await target.evaluate((el) => el.matches(".skip-link"));
      if (isHiddenSkipLink) {
        continue;
      }
    }
    // HIT-TEST across the FULL area, not just the centre: an overlay covering the
    // control still lets `boundingBox()` report its full size, and an overlay that
    // covers almost everything but leaves a small central hole would pass a
    // centre-only probe while most taps land on the overlay. Probe a PLUS of five
    // points — the centre plus the four edge-midpoints, inset 15% so they sit on the
    // flat edges (avoiding rounded corners, whose transparent pixels legitimately
    // resolve to the background) — and require EVERY point to resolve to the control
    // or a DESCENDANT (a pseudo-element hit reports its originating element). An
    // ANCESTOR is rejected: a `pointer-events: none` control returns its container,
    // which would falsely certify an untappable target.
    const hits = await target.evaluate((el) => {
      const r = el.getBoundingClientRect();
      const points: [number, number][] = [
        [0.5, 0.5],
        [0.5, 0.15],
        [0.5, 0.85],
        [0.15, 0.5],
        [0.85, 0.5],
      ];
      let reachable = 0;
      for (const [fx, fy] of points) {
        const h = document.elementFromPoint(r.left + r.width * fx, r.top + r.height * fy);
        if (h !== null && (el === h || (h instanceof Node && el.contains(h)))) reachable += 1;
      }
      return { reachable, total: points.length };
    });
    expect(
      hits.reachable,
      `${where}: a target is partly covered — only ${hits.reachable}/${hits.total} probe points across it reach it`,
    ).toBe(hits.total);
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

test("populated room rows meet the compact target-size floors", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name !== "compact" && testInfo.project.name !== "narrow",
    "target-size floors are the compact/narrow contract (§7)",
  );
  // The empty shell renders NO room rows, so the settled-shell sweep never measures
  // `.room-select` — a compact room row was ~38px, below the 44px floor, while the
  // matrix stayed green. Populate the list so the real row geometry is under test.
  await gotoReadyShellWithRooms(page, 3);
  await expect(page.locator(".room-select")).toHaveCount(3);
  await assertTargetGeometry(page, "populated room list");
});

test("a compact control regressed below the floor is caught, not silently skipped", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name !== "compact" && testInfo.project.name !== "narrow",
    "target-size floors are the compact/narrow contract (§7)",
  );
  await gotoReadyShell(page);
  // Inject a REAL interactive control collapsed to 1px that is NOT the hidden
  // skip-link pattern — a CSS regression. The ≤2px skip now exempts ONLY `.skip-link`,
  // so this must FAIL the geometry check (24px floor / hit-test), not be silently
  // excused, keeping the accessibility matrix honest. Red-before: the old blanket
  // ≤2px skip excused it and assertTargetGeometry passed.
  await page.evaluate(() => {
    const b = document.createElement("button");
    b.textContent = "x";
    b.setAttribute(
      "style",
      // border-box + zeroed padding/border/appearance so the box is a TRUE 1px — the
      // ≤2px branch the exemption guards (UA button padding would otherwise inflate it
      // past the branch and it would fail the floor for an unrelated reason).
      "appearance:none;box-sizing:border-box;margin:0;padding:0;border:0;position:fixed;bottom:0;right:0;width:1px;height:1px;min-width:0;min-height:0;overflow:hidden;z-index:9999",
    );
    document.body.appendChild(b);
  });
  await expect(assertTargetGeometry(page, "injected 1px control")).rejects.toThrow();
});

test("a target covered except its centre is caught, not certified by a centre-only probe", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name !== "compact" && testInfo.project.name !== "narrow",
    "target-size floors are the compact/narrow contract (§7)",
  );
  await gotoReadyShell(page);
  // A full-size 44×44 control whose TOP band is covered by an overlay, leaving the
  // centre exposed. A centre-only hit-test would certify it; the multi-point probe
  // sees the covered edge and fails. Red-before: only the centre was probed.
  await page.evaluate(() => {
    const b = document.createElement("button");
    b.textContent = "x";
    b.setAttribute(
      "style",
      "appearance:none;box-sizing:border-box;position:fixed;top:120px;left:120px;width:44px;height:44px;margin:0;padding:0;border:0;z-index:9000",
    );
    document.body.appendChild(b);
    const overlay = document.createElement("div");
    overlay.setAttribute(
      "style",
      "position:fixed;top:120px;left:120px;width:44px;height:16px;background:red;z-index:9999",
    );
    document.body.appendChild(overlay);
  });
  await expect(assertTargetGeometry(page, "partly-covered target")).rejects.toThrow();
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
