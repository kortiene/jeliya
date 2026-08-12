import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

import {
  attachJson,
  expectCleanNetwork,
  gotoReadyShell,
  installNoNetworkGuard,
  openDiagnostics,
  type NetworkGuard,
} from "./a11y-harness";

// The #177 axe sweep (§7): every foundation state, at all four viewport
// projects, with the full tag set — failing on any CRITICAL or SERIOUS
// violation, attaching moderate/minor findings as advisory evidence.
//
// Foundation states swept: the settled landmarked shell (empty rooms answer,
// empty center, skip links, live region, status footer) and the Diagnostics
// dialog. The boot cover is NOT deterministically reachable in the shipped
// artifact — the mock composition drives Ready without an externally
// observable hold point — so sweeping it waits for a fault-injectable
// composition (spec §7 records this).

const TAGS = [
  "wcag2a",
  "wcag2aa",
  "wcag21a",
  "wcag21aa",
  "wcag22aa",
  "best-practice",
];

// Documented false positives, each entry shaped
// { id, selector, rationale, link } — the guard test below refuses an entry
// without a linked rationale, so this list cannot silently grow into an
// unaudited mute button. Empty today: the foundation passes clean.
const FALSE_POSITIVES: {
  id: string;
  selector: string;
  rationale: string;
  link: string;
}[] = [];

test("every documented false positive carries a linked rationale", () => {
  for (const entry of FALSE_POSITIVES) {
    expect(entry.id, "a false positive names the axe rule it mutes").not.toBe("");
    expect(entry.selector, "a false positive is scoped to a selector, never rule-wide").not.toBe("");
    expect(
      entry.rationale.length,
      "a false positive explains WHY the finding is wrong",
    ).toBeGreaterThanOrEqual(20);
    expect(
      entry.link,
      "a false positive links its tracking issue or upstream report",
    ).toMatch(/^https?:\/\/|#\d+/);
  }
});

let guard: NetworkGuard;

test.beforeEach(async ({ page, baseURL }) => {
  guard = await installNoNetworkGuard(page, baseURL);
});

test.afterEach(() => {
  expectCleanNetwork(guard);
});

async function sweep(
  page: Parameters<typeof gotoReadyShell>[0],
  testInfo: Parameters<typeof attachJson>[0],
  state: string,
): Promise<void> {
  const results = await new AxeBuilder({ page }).withTags(TAGS).analyze();
  const violations = results.violations.filter(
    (violation) =>
      !FALSE_POSITIVES.some(
        (fp) =>
          fp.id === violation.id &&
          violation.nodes.every((node) => node.target.join(" ").includes(fp.selector)),
      ),
  );
  const blocking = violations.filter(
    (violation) => violation.impact === "critical" || violation.impact === "serious",
  );
  const advisory = violations.filter(
    (violation) => violation.impact !== "critical" && violation.impact !== "serious",
  );
  if (advisory.length > 0) {
    await attachJson(testInfo, `${state}-advisory-violations`, advisory);
  }
  expect(
    blocking.map((violation) => ({
      id: violation.id,
      impact: violation.impact,
      nodes: violation.nodes.map((node) => node.target.join(" ")),
    })),
    `${state}: no critical/serious axe violation may ship in the foundation`,
  ).toEqual([]);
}

test("the settled shell passes the axe sweep", async ({ page }, testInfo) => {
  await gotoReadyShell(page);
  await sweep(page, testInfo, "settled-shell");
});

test("the Diagnostics dialog passes the axe sweep", async ({ page }, testInfo) => {
  await gotoReadyShell(page);
  await openDiagnostics(page);
  await sweep(page, testInfo, "diagnostics-dialog");
});
