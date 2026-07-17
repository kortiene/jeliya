import { defineConfig, devices } from '@playwright/test';

// Browser-level UX regression harness (issue #51). Runs the real app against
// the VITE_MOCK=1 fixture client — no daemon, no network — so every flow is
// deterministic. Four viewport projects cover the responsive contract:
// the two desktop grids (wide + narrowed columns) and the two compact
// (max-width: 900px) phone layouts the mockups target.
//
// Run with `npm run test:e2e` (boots its own dev server).
//
// The port is deliberately NOT a default of any other tool (vite dev's 5173,
// vite preview's 4173): with reuseExistingServer enabled locally, a colliding
// port would let the suite silently attach to a non-mock server — worst case
// driving a real daemon, where the onboarding spec would create a real,
// irreversible identity. Belt and braces: every spec also refuses to run
// unless the app under test reports the mock transport (see e2e/fixtures.ts).

const PORT = 43117;

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  // No retries: a test that only passes on retry is a regression this harness
  // exists to catch, not to paper over.
  retries: 0,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : [['list']],
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    // Failures must preserve the full evidence trail (screenshot, trace,
    // console errors — the latter attached by the fixture in e2e/fixtures.ts).
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    // The app honors prefers-reduced-motion (a WCAG contract, not a hint),
    // and the suite runs under it: animations — including the timeline's
    // jump-to-latest scroll — settle instantly, so assertions never race an
    // in-flight animation against the mock's live-event timers.
    contextOptions: { reducedMotion: 'reduce' },
  },
  projects: [
    {
      name: 'desktop-1440x900',
      use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } },
    },
    {
      name: 'desktop-920x800',
      use: { ...devices['Desktop Chrome'], viewport: { width: 920, height: 800 } },
    },
    {
      name: 'mobile-390x844',
      use: { ...devices['Desktop Chrome'], viewport: { width: 390, height: 844 }, hasTouch: true },
    },
    {
      name: 'mobile-320x568',
      use: { ...devices['Desktop Chrome'], viewport: { width: 320, height: 568 }, hasTouch: true },
    },
  ],
  webServer: {
    command: `npm run dev -- --host 127.0.0.1 --port ${PORT} --strictPort`,
    url: `http://127.0.0.1:${PORT}`,
    env: { VITE_MOCK: '1' },
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});
