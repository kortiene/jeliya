import { expect, test } from '@playwright/test';

// Runtime measurements for acceptance criterion 4. These are SPIKE numbers on
// one machine, from a debug daemon, with no wasm-opt — directionally useful,
// not a budget. #198 sets the actual first-release budgets.
test('record bootstrap and interaction timings', async ({ page }) => {
  await page.goto('/', { waitUntil: 'commit' });

  await expect(page.locator('#spike-app')).toBeVisible({ timeout: 30_000 });

  const nav = await page.evaluate(() => {
    const [entry] = performance.getEntriesByType('navigation') as PerformanceNavigationTiming[];
    const paints = performance.getEntriesByType('paint');
    const wasm = performance
      .getEntriesByType('resource')
      .filter((r) => r.name.endsWith('.wasm'))
      .map((r) => ({ transferSize: (r as PerformanceResourceTiming).transferSize, duration: r.duration }));
    return {
      responseEnd: entry?.responseEnd ?? null,
      domContentLoaded: entry?.domContentLoadedEventEnd ?? null,
      firstPaint: paints.find((p) => p.name === 'first-paint')?.startTime ?? null,
      firstContentfulPaint: paints.find((p) => p.name === 'first-contentful-paint')?.startTime ?? null,
      wasm,
      // Chromium-only, and an approximation by design.
      heapUsed: (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize ?? null,
    };
  });

  // Time from navigation to the app being interactive: wasm fetch + instantiate
  // + the daemon round trips (session, ws, daemon.status, identity.create,
  // room.list, room.create, room.list).
  const readyAt = await page.evaluate(() => performance.now());

  // Send-to-rendered-push, measured in the page.
  const t0 = await page.evaluate(() => performance.now());
  await page.locator('.room-select').first().click();
  await page.locator('#composer-input').fill('measurement');
  await page.locator('#spike-send').click();
  await expect(page.locator('.msg-bubble')).toHaveText(['measurement']);
  const t1 = await page.evaluate(() => performance.now());

  const report = {
    ...nav,
    appInteractiveMs: Math.round(readyAt),
    openSendPushMs: Math.round(t1 - t0),
  };
  console.log(`\nSPIKE_MEASUREMENTS ${JSON.stringify(report, null, 2)}\n`);

  // Not budgets — just loud failure if something regresses by an order of
  // magnitude while the spike is still being iterated on.
  expect(report.appInteractiveMs).toBeLessThan(15_000);
});
