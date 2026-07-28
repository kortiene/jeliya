import { defineConfig, devices } from '@playwright/test';

// The daemon serves the spike's own `dist` on a fixed loopback port, so the page
// is same-origin with the control surface — the path the spike exists to prove.
const PORT = Number(process.env.SPIKE_PORT ?? 7458);
const BASE = `http://127.0.0.1:${PORT}`;

export default defineConfig({
  testDir: './e2e',
  // A feasibility spike that passes only sometimes has not proved feasibility.
  retries: 0,
  workers: 1,
  reporter: [['list']],
  use: {
    baseURL: BASE,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
    ...devices['Desktop Chrome'],
  },
  webServer: {
    // SPIKE_EMBEDDED=1 offers the daemon no directory, so an `embed-ui` build
    // must serve the assets compiled into it or the page 404s.
    command: `node fixture.mjs --port ${PORT}${process.env.SPIKE_EMBEDDED ? ' --no-ui-dir' : ''}`,
    // /api/health is unauthenticated, so it is a real readiness signal.
    url: `${BASE}/api/health`,
    reuseExistingServer: false,
    timeout: 60_000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
