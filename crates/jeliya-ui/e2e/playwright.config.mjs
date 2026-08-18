import { defineConfig, devices } from "@playwright/test";

// The render smoke serves the reproducible `dist/` statically on loopback and
// loads it with NO network access — the offline "does the shell render against
// the shared design system" check (§11, Verification). Mirrors
// spikes/dioxus-web/e2e but needs no daemon: #176 renders against the mock.
const PORT = Number(process.env.RENDER_PORT ?? 7461);
const BASE = `http://127.0.0.1:${PORT}`;

// The #177 accessibility matrix runs the a11y specs at four viewport
// contracts (§7): the stylesheet's wide/medium desktop layouts, the compact
// pane-select layout, and the narrowest supported phone. Reduced motion is
// forced by default — the WCAG-relevant setting; the foundation shell carries
// no motion-bearing element yet (the three animated elements in styles.css
// are React chat surfaces), so there is no branch to prove different until
// one exists.
const A11Y_VIEWPORTS = {
  wide: { width: 1440, height: 900 },
  medium: { width: 920, height: 1000 },
  compact: { width: 390, height: 844 },
  narrow: { width: 320, height: 568 },
};

export default defineConfig({
  testDir: ".",
  // A smoke that passes only sometimes has not smoked anything.
  retries: 0,
  workers: 1,
  reporter: [["list"]],
  use: {
    baseURL: BASE,
    // Fail closed on any outbound request other than to the loopback origin:
    // the artifact must render with no network.
    ...devices["Desktop Chrome"],
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    // The #176 offline render smoke, unchanged: one desktop viewport (its own
    // compact describe overrides per-test), so its contract is exactly what
    // shipped with the foundation.
    { name: "render", testMatch: /render\.spec\.ts$/ },
    // The #178 shell/routing/preference smoke: URL canonicalization, malformed-URL
    // recovery, in-app navigation, room deep link, responsive nav topology, and
    // legacy key purge. Runs at the default Desktop Chrome viewport; compact/wide
    // topology tests override per-describe via test.use({ viewport }).
    { name: "shell", testMatch: /shell-178\.spec\.ts$/ },
    // The #179 Activity pane smoke: pane structure, loading state, filter chip
    // interactions, composer keyboard contract (desktop enter hint / compact
    // suppression), and per-room draft persistence across in-app route changes.
    // Runs at the default Desktop Chrome viewport; the compact describe overrides.
    { name: "activity", testMatch: /activity-179\.spec\.ts$/ },
    ...Object.entries(A11Y_VIEWPORTS).map(([name, viewport]) => ({
      name,
      testMatch: /a11y[^/]*\.spec\.ts$/,
      use: { viewport, reducedMotion: "reduce" },
    })),
  ],
  webServer: {
    command: `node serve.mjs`,
    url: BASE,
    reuseExistingServer: false,
    timeout: 30_000,
    env: { RENDER_PORT: String(PORT) },
  },
});
