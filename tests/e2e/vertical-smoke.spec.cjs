const { expect, test } = require('../../web/node_modules/@playwright/test');
const { randomUUID } = require('node:crypto');

const enabled = process.env.NOCTIS_VERTICAL_E2E === '1';
const projectId = process.env.NOCTIS_E2E_PROJECT_ID;
const projectRunId = process.env.NOCTIS_E2E_PROJECT_RUN_ID;

test.describe('production vertical slice', () => {
  test.skip(!enabled, 'set NOCTIS_VERTICAL_E2E=1 after M2-014 production wiring is integrated');
  test.skip(!projectId || !projectRunId, 'seed project ownership and set fixture UUIDs');

  test('provider to terminal task result survives SSE reconnect', async ({ page, request }) => {
    const suffix = Date.now().toString(36);
    const providerId = `e2e-provider-${suffix}`;
    const modelId = `e2e-model-${suffix}`;
    const taskId = `e2e-task-${suffix}`;
    const headers = () => ({ 'Idempotency-Key': randomUUID() });

    const provider = await request.post('/api/v1/providers', {
      headers: headers(),
      data: {
        id: providerId,
        base_url: 'http://127.0.0.1:7411/v1',
        api_key_env: 'PRIMARY_API_KEY',
        request_timeout_seconds: 10
      }
    });
    expect(provider.ok()).toBeTruthy();
    try {
      const model = await request.post(`/api/v1/providers/${providerId}/models`, {
        headers: headers(),
        data: {
          id: modelId,
          provider_id: providerId,
          remote_name: 'fixture-model',
          class: 'coding',
          context_window: 8192,
          max_output_tokens: 2048,
          claimed_capabilities: { chat: true, streaming: true, tools: true, parallel_tools: false }
        }
      });
      expect(model.ok()).toBeTruthy();

      await page.goto('/providers');
      await expect(page.getByText(providerId, { exact: true })).toBeVisible();
      await page.getByRole('button', { name: new RegExp(providerId) }).press('Enter');
      await expect(page.getByText(modelId, { exact: true })).toBeVisible();

      await page.goto('/tasks');
      await expect(page.getByRole('heading', { name: 'Tasks', exact: true })).toBeVisible();
      await expect(page.getByText('Loading tasks…')).toHaveCount(0);
      await page.keyboard.press('Tab');
      await expect(page.getByRole('button', { name: 'Create task' })).toBeFocused();
      await page.keyboard.press('Enter');
      const form = page.getByRole('form', { name: 'Create task' });
      await expect(form).toBeVisible();
      await form.getByLabel('Task ID').fill(taskId);
      await form.getByLabel('Title').fill('Playwright vertical smoke');
      await form.getByLabel('Project UUID').fill(projectId);
      await form.getByLabel('Project run UUID').fill(projectRunId);
      await form.getByLabel('Role').fill('WebApp Engineer');
      await form.getByLabel('Objective').fill('Exercise production vertical flow.');
      await form.getByLabel('Allowed paths, one per line').fill('e2e-output.txt');
      await form.getByLabel('Acceptance criteria, one per line').fill('Terminal result is visible.');
      await form.getByLabel('Verification commands, one per line').fill('git diff --check');
      await form.getByRole('button', { name: 'Create task' }).press('Enter');
      await expect(page.getByRole('status')).toContainText(taskId);

      await page.getByRole('link', { name: /Playwright vertical smoke/ }).press('Enter');
      await expect(page).toHaveURL(new RegExp(`/tasks/${taskId}$`));
      await page.getByRole('button', { name: 'Start' }).press('Enter');
      await expect(page.getByRole('status')).toContainText('Start accepted');
      await page.getByRole('button', { name: 'Start' }).press('Enter');
      const visibleFailure = page.getByRole('alert');
      await expect(visibleFailure).toBeVisible();
      const failureText = await visibleFailure.textContent();

      const eventIdsBefore = await page.locator('.timeline > li > span').allTextContents();
      await page.context().setOffline(true);
      await expect(page.getByText('Live connection interrupted. Browser is reconnecting.')).toBeVisible();
      await page.context().setOffline(false);

      await expect(page.locator('.status')).toContainText(/DONE|FAILED/, { timeout: 120_000 });
      const eventIds = await page.locator('.timeline > li > span').allTextContents();
      expect(new Set(eventIds).size).toBe(eventIds.length);
      expect(eventIds.length).toBeGreaterThanOrEqual(eventIdsBefore.length);
      await expect(page.getByRole('heading', { name: 'Verification results' })).toBeVisible();
      await expect(page.getByText(/passed|exit_code/i)).toBeVisible();
      await expect(page.getByRole('heading', { name: 'Artifacts' })).toBeVisible();
      await expect(page.getByRole('link', { name: 'Download' })).toBeVisible();
      await expect(page.getByRole('heading', { name: 'Diff' })).toBeVisible();
      await expect(page.getByLabel('Task diff')).toContainText('e2e-output.txt');
      await expect(page.getByRole('heading', { name: 'Usage' })).toBeVisible();
      await expect(page.locator('.metrics dd').first()).not.toHaveText('0');
      await expect(visibleFailure).toContainText(failureText ?? '');
      await expect(page.locator('main')).not.toContainText(process.env.NOCTIS_E2E_API_KEY);
      await expect(page.locator('main')).not.toContainText('/home/');
      await expect(page.locator('main')).not.toContainText('chat.completion');
    } finally {
      await request.delete(`/api/v1/providers/${providerId}`, { headers: headers() });
    }
  });
});
