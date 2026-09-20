const { expect, test } = require('../../web/node_modules/@playwright/test');

test('keyboard reaches Providers and primary Tasks control', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'AI Team' })).toBeVisible();

  await page.keyboard.press('Tab');
  await expect(page.getByRole('link', { name: 'Kelola providers' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/providers$/);
  await expect(page.getByRole('heading', { name: 'Providers', exact: true })).toBeVisible();

  const tasksResponse = page.waitForResponse(
    (response) =>
      response.request().method() === 'GET' && new URL(response.url()).pathname === '/api/v1/tasks'
  );
  await page.goto('/tasks');
  await tasksResponse;
  await expect(page.getByRole('heading', { name: 'Tasks', exact: true })).toBeVisible();
  await expect(page.getByText('Loading tasks…')).toHaveCount(0);
  await page.keyboard.press('Tab');
  const createTask = page.getByRole('button', { name: 'Create task' });
  await expect(createTask).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.locator('form[aria-label="Create task"]')).toBeVisible();
});
