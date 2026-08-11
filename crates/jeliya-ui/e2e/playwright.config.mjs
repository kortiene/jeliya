import { defineConfig, devices } from "@playwright/test";

// The render smoke serves the reproducible `dist/` statically on loopback and
// loads it with NO network access — the offline "does the shell render against
// the shared design system" check (§11, Verification). Mirrors
// spikes/dioxus-web/e2e but needs no daemon: #176 renders against the mock.
const PORT = Number(process.env.RENDER_PORT ?? 7461);
const BASE = `http://127.0.0.1:${PORT}`;

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
  webServer: {
    command: `node serve.mjs`,
    url: BASE,
    reuseExistingServer: false,
    timeout: 30_000,
    env: { RENDER_PORT: String(PORT) },
  },
});
