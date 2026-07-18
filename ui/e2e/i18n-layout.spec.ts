import type { Locator, Page } from '@playwright/test';
import type { Catalog } from '../src/l10n/catalog';
import type { SupportedLocale } from '../src/l10n/locale';
import { en } from '../src/l10n/en';
import { fr } from '../src/l10n/fr';
import { expect, MOCK_ROOMS, test } from './fixtures';

/** Issue #74: the critical-flow reflow gate in every shipped text locale.
 *
 * Browser zoom changes the CSS viewport available to the page. A 1280px-wide
 * browser at 400% zoom exposes 320 CSS px, which is also WCAG 1.4.10's reflow
 * target. Playwright cannot drive Chromium's browser chrome zoom control, so
 * this uses the equivalent 320px CSS viewport, matching responsive.spec.ts.
 * Running from the wide desktop project keeps this a browser-zoom test rather
 * than silently adding touch-device behavior to the matrix.
 */
test.skip(({ viewport }) => (viewport?.width ?? 0) !== 1440, 'viewport-driven: one project is enough');

const REFLOW_VIEWPORT = { width: 320, height: 568 } as const;

const LOCALES: readonly { tag: SupportedLocale; strings: Catalog }[] = [
  { tag: 'en', strings: en },
  { tag: 'fr', strings: fr },
];

async function useLocale(page: Page, locale: SupportedLocale): Promise<void> {
  await page.addInitScript((tag: string) => {
    localStorage.setItem('jeliya.textLocale', tag);
    localStorage.setItem('jeliya.formattingLocale', tag);
  }, locale);
}

async function expectMockBuild(page: Page): Promise<void> {
  await expect(
    page.locator('html'),
    'the layout matrix must never drive a real daemon',
  ).toHaveAttribute('data-jeliya-transport', /mock fixtures \(VITE_MOCK=1\)/);
}

async function expectLocale(page: Page, locale: SupportedLocale): Promise<void> {
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}

async function expectNoHorizontalOverflow(surface: Locator, name: string): Promise<void> {
  const width = await surface.evaluate((element) => ({
    client: element.clientWidth,
    scroll: element.scrollWidth,
  }));
  expect(
    width.scroll,
    `${name} spills horizontally: scrollWidth ${width.scroll}px > clientWidth ${width.client}px`,
  ).toBeLessThanOrEqual(width.client + 1);
}

/** Prove a control is reachable by ordinary vertical scrolling and finishes
 * wholly inside the visual viewport, rather than merely existing below a
 * clipped fixed-height surface. */
async function expectReachable(page: Page, control: Locator, name: string): Promise<void> {
  await control.scrollIntoViewIfNeeded();
  await expect(control).toBeVisible();
  await expect(control).toBeInViewport();

  const box = await control.boundingBox();
  expect(box, `${name} has no rendered box`).not.toBeNull();
  if (!box) return;

  expect(box.x, `${name} starts left of the viewport`).toBeGreaterThanOrEqual(-1);
  expect(box.x + box.width, `${name} ends right of the viewport`).toBeLessThanOrEqual(REFLOW_VIEWPORT.width + 1);
  expect(box.y, `${name} starts above the viewport`).toBeGreaterThanOrEqual(-1);
  expect(box.y + box.height, `${name} ends below the viewport`).toBeLessThanOrEqual(REFLOW_VIEWPORT.height + 1);

  // Settings has fixed bottom chrome. Intersecting the viewport is not enough
  // if that chrome paints over the focused action.
  const tabBar = page.locator('.mobile-tabbar');
  if (await tabBar.isVisible()) {
    const tabBox = await tabBar.boundingBox();
    if (tabBox) {
      expect(box.y + box.height, `${name} is covered by the compact tab bar`).toBeLessThanOrEqual(tabBox.y + 1);
    }
  }
}

for (const { tag, strings } of LOCALES) {
  test.describe(`${tag} at the 320px / 400%-zoom reflow target`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize(REFLOW_VIEWPORT);
      await useLocale(page, tag);
    });

    test('onboarding copy and primary action reflow', async ({ page }) => {
      await page.goto('/?mock=fresh');
      await expectMockBuild(page);

      const onboarding = page.locator('#onboarding-main');
      const card = onboarding.locator('.onboarding-card');
      const primaryAction = card.locator('button.btn-primary');

      await expect(onboarding).toBeVisible();
      await expectLocale(page, tag);
      await expect(card.locator('h2')).toHaveText(strings.onboardingIdentityTitle);
      await expect(primaryAction).toHaveText(strings.onboardingCreateIdentity);
      await expectNoHorizontalOverflow(page.locator('html'), `${tag} onboarding document`);
      await expectNoHorizontalOverflow(onboarding, `${tag} onboarding surface`);
      await expectReachable(page, primaryAction, `${tag} onboarding primary action`);

      // The second onboarding step has the denser and longer copy: two forms,
      // the identity handoff, and French field labels. Reach it through the
      // real mock action so the gate covers the whole critical flow.
      await primaryAction.click();
      const roomsStep = onboarding.locator('.onboarding-rooms');
      const forms = roomsStep.locator('form.onboarding-card');
      const createRoom = forms.first().locator('button.btn-primary');
      const joinRoom = forms.nth(1).locator('button.btn-primary');

      await expect(roomsStep).toBeVisible();
      await expect(forms.first().locator('h2')).toHaveText(strings.modalCreateTitle);
      await expect(forms.nth(1).locator('h2')).toHaveText(strings.roomsJoinWithTicket);
      await expect(createRoom).toHaveText(strings.roomsCreate);
      await expect(joinRoom).toHaveText(strings.modalJoinSubmit);
      await expectNoHorizontalOverflow(page.locator('html'), `${tag} room onboarding document`);
      await expectNoHorizontalOverflow(onboarding, `${tag} room onboarding surface`);
      await expectReachable(page, createRoom, `${tag} create-first-room action`);
      await expectReachable(page, joinRoom, `${tag} join-first-room action`);
    });

    test('populated room shell keeps the timeline and composer usable', async ({ app, page }) => {
      await app.gotoPopulated();
      await expectMockBuild(page);

      const workspace = page.locator('#workspace');
      const activityTab = page.locator('#room-tab-activity');
      const composer = page.locator('#composer-input');

      await expectLocale(page, tag);
      await expect(workspace.locator('.appbar-title h1')).toHaveText(MOCK_ROOMS.main);
      await expect(activityTab.locator('.panel-tab-label')).toHaveText(strings.roomDestActivity);
      await expect(app.timeline).toBeVisible();
      await expect(composer).toHaveAttribute('placeholder', strings.composerMessagePlaceholder(MOCK_ROOMS.main));
      await expectNoHorizontalOverflow(page.locator('html'), `${tag} shell document`);
      await expectNoHorizontalOverflow(workspace, `${tag} room workspace`);
      await expectReachable(page, composer, `${tag} message composer`);
    });

    test('settings language controls and primary action reflow', async ({ page }) => {
      await page.goto('/settings');
      await expectMockBuild(page);

      const settings = page.locator('#settings-main');
      const localeCard = settings.locator('.settings-locale-card');
      const selects = localeCard.locator('select');
      const createRoom = settings.locator(':scope > button.btn-primary');

      await expect(settings).toBeVisible();
      await expectLocale(page, tag);
      await expect(settings.locator('#settings-title')).toHaveText(strings.settingsTitle);
      await expect(localeCard.locator('label').first().locator('span')).toHaveText(strings.settingsLanguageLabel);
      await expect(localeCard.locator('label').nth(1).locator('span')).toHaveText(strings.settingsFormattingLabel);
      await expect(selects.first()).toHaveValue(tag);
      await expect(selects.nth(1)).toHaveValue(tag);
      await expect(createRoom).toHaveText(strings.roomsCreate);
      await expectNoHorizontalOverflow(page.locator('html'), `${tag} settings document`);
      await expectNoHorizontalOverflow(settings, `${tag} settings surface`);
      await expectReachable(page, selects.first(), `${tag} language selector`);
      await expectReachable(page, createRoom, `${tag} settings primary action`);
    });
  });
}

test('text and formatting preferences switch live and persist independently', async ({ app, page }) => {
  await page.goto('/settings');
  await expectMockBuild(page);

  const settings = page.locator('#settings-main');
  const selects = settings.locator('.settings-locale-card select');

  await selects.first().selectOption('fr');
  await expectLocale(page, 'fr');
  await expect(settings.locator('#settings-title')).toHaveText(fr.settingsTitle);

  // Formatting changes independently: the interface stays French while a
  // real fractional byte size follows the selected numeric convention.
  await selects.nth(1).selectOption('fr');
  await app.gotoPopulated();
  await page.locator('.room-select', { hasText: MOCK_ROOMS.main }).click();
  await page.locator('#room-tab-files').click();
  const prdMeta = app.rightPanel.locator('.file-row', { hasText: 'PRD_v0.2.pdf' }).locator('.file-meta');
  await expect(prdMeta).toContainText('1,8 Mo');

  await page.goto('/settings');
  await selects.nth(1).selectOption('en');
  await expectLocale(page, 'fr');
  await app.gotoPopulated();
  await page.locator('.room-select', { hasText: MOCK_ROOMS.main }).click();
  await page.locator('#room-tab-files').click();
  await expect(app.rightPanel.locator('.file-row', { hasText: 'PRD_v0.2.pdf' }).locator('.file-meta')).toContainText(
    '1.8 Mo',
  );
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem('jeliya.textLocale')))
    .toBe('fr');
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem('jeliya.formattingLocale')))
    .toBe('en');

  await page.goto('/settings');
  await page.reload();
  await expectMockBuild(page);
  await expectLocale(page, 'fr');
  await expect(selects.first()).toHaveValue('fr');
  await expect(selects.nth(1)).toHaveValue('en');

  // Clearing the choices removes both storage keys. Chromium's test platform
  // advertises English, so the system-following text fallback resolves to en.
  await selects.first().selectOption('');
  await selects.nth(1).selectOption('');
  await expectLocale(page, 'en');
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem('jeliya.textLocale')))
    .toBeNull();
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem('jeliya.formattingLocale')))
    .toBeNull();
});
