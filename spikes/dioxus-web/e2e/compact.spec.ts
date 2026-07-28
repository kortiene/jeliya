import { expect, test } from '@playwright/test';

// The CSS-reuse claim was desktop-only until this existed.
//
// `ui/src/styles.css` is a one-pane-at-a-time shell below 899.98px: it sets
// `.app .sidebar` and `.app .center` to `display: none` and reveals exactly one
// through a root pane state. A slice whose root is plain `app` renders a blank
// screen on every compact viewport — including a phone system WebView, which is
// a target platform. Desktop Chrome cannot see that, so this asserts it at the
// two compact widths the repo already tests (CONTRIBUTING.md).
for (const viewport of [
  { name: '390x844', width: 390, height: 844 },
  { name: '320x568', width: 320, height: 568 },
]) {
  test(`the reused compact shell reveals one pane at a time (${viewport.name})`, async ({ page }) => {
    await page.setViewportSize({ width: viewport.width, height: viewport.height });
    await page.goto('/');

    const app = page.locator('#spike-app');
    await expect(app).toBeVisible({ timeout: 30_000 });

    // No room open: the rooms pane is the visible one and the workspace is not.
    await expect(app).toHaveClass(/pane-rooms/);
    await expect(page.locator('.sidebar')).toBeVisible();
    await expect(page.locator('.center')).toBeHidden();

    await page.locator('.room-select').first().click();

    // Opening a room switches panes: the workspace shows, the list hides.
    await expect(app).toHaveClass(/pane-room/);
    await expect(page.locator('.center')).toBeVisible();
    await expect(page.locator('.sidebar')).toBeHidden();
    await expect(page.locator('#composer-input')).toBeVisible();

    // The pane is genuinely laid out, not merely present with zero area.
    const box = await page.locator('.center').boundingBox();
    expect(box?.width ?? 0).toBeGreaterThan(0);
    expect(box?.height ?? 0).toBeGreaterThan(0);
  });
}
