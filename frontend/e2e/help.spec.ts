import { test, expect, openScreen } from './fixtures';

/**
 * The help in a real browser: `?` opens the help of the screen on display,
 * a card's "?" opens its bubble beside it and inside the window, the search
 * reaches every screen's help, and the quick search finds it too.
 */

test('? opens the help of the screen on display, and Escape gives the focus back', async ({
  page,
}) => {
  await openScreen(page, '/rules');
  await page.locator('body').press('?');

  const help = page.getByRole('dialog', { name: 'Help: Rules' });
  await expect(help).toBeVisible();
  await expect(help.getByRole('searchbox', { name: 'Search the help' })).toBeFocused();
  await expect(help.getByRole('heading', { level: 3, name: 'Rules' })).toBeVisible();

  await page.keyboard.press('Escape');
  await expect(help).toBeHidden();

  // From a field, `?` is a character.
  await page.getByRole('button', { name: 'Help', exact: true }).click();
  await expect(help).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Help', exact: true })).toBeFocused();
});

test("a card's ? opens its help beside it, inside the window, and Escape closes it", async ({
  page,
}) => {
  await openScreen(page, '/categories');
  const toggle = page.getByRole('button', { name: 'Help: Folder mappings' });
  await toggle.click();

  const bubble = page.locator('.help-bubble:popover-open');
  await expect(bubble).toBeVisible();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  const [anchor, box] = await Promise.all([toggle.boundingBox(), bubble.boundingBox()]);
  const width = page.viewportSize()!.width;
  expect(box!.y).toBeGreaterThanOrEqual(anchor!.y + anchor!.height);
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(width);

  await page.keyboard.press('Escape');
  await expect(bubble).toBeHidden();
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
});

test("the help's search reaches another screen's help, and opens it", async ({ page }) => {
  await openScreen(page, '/');
  await page.getByRole('button', { name: 'Help', exact: true }).click();
  const help = page.getByRole('dialog', { name: /^Help: / });

  await help.getByRole('searchbox', { name: 'Search the help' }).fill('webhook token');
  await help
    .getByRole('button', { name: /Webhook token/ })
    .first()
    .click();

  await expect(help.getByRole('heading', { level: 3, name: 'Instances' })).toBeVisible();
});

test('the quick search finds the help, and opens it on its screen', async ({ page }) => {
  await openScreen(page, '/');
  await page.getByRole('button', { name: 'Quick search' }).click();
  await page.getByRole('combobox', { name: 'Quick search' }).fill('batch limit');

  const group = page.getByRole('group', { name: 'Help' });
  await group.getByRole('option').first().click();

  await expect(page.getByRole('dialog', { name: /^Help: / })).toBeVisible();
});
