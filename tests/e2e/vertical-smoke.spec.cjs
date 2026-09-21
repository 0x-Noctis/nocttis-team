const { expect, test } = require('../../web/node_modules/@playwright/test');
const { randomUUID } = require('node:crypto');
const { spawnSync } = require('node:child_process');

const projectId = '00000000-0000-4000-8000-000000000015';
const projectRunId = '00000000-0000-4000-8000-000000000016';

function makeReady(taskId) {
  const sql = `UPDATE tasks SET status='READY', version=version+1 WHERE id='${taskId}' AND status='DRAFT'`;
  const result = spawnSync('docker', [
    'exec', '-i', 'noctis-agent-3-e2e-postgres', 'psql', '-U', 'ai_team', '-d', 'ai_team',
    '-v', 'ON_ERROR_STOP=1', '-c', sql
  ]);
  expect(result.status).toBe(0);
}

test.describe('production vertical slice', () => {
  test('provider to terminal task result survives SSE reconnect', async ({ page, request }) => {
    test.setTimeout(150_000);
    const suffix = Date.now().toString(36);
    const providerId = `e2e-provider-${suffix}`;
    const modelId = 'e2e-model';
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
      const probe = await request.post(`/api/v1/models/${modelId}/probes/tools`, {
        headers: headers(),
        data: {}
      });
      expect(probe.ok()).toBeTruthy();
      expect((await probe.json()).result.verified).toBe('supported');

      await page.goto('/providers');
      const providerButton = page.getByRole('button', { name: new RegExp(providerId) });
      await expect(providerButton).toBeVisible();
      await providerButton.press('Enter');
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
      await form.getByLabel('Verification commands, one per line').fill('true');
      await form.getByRole('button', { name: 'Create task' }).press('Enter');
      await expect(page.getByRole('status')).toContainText(taskId);
      makeReady(taskId);

      await page.getByRole('link', { name: /Playwright vertical smoke/ }).press('Enter');
      await expect(page).toHaveURL(new RegExp(`/tasks/${taskId}$`));
      await page.getByRole('button', { name: 'Start' }).press('Enter');
      await expect(page.getByRole('status')).toContainText('Start accepted');

      await expect(page.locator('.timeline > li')).not.toHaveCount(0);
      const eventIdsBefore = await page.locator('.timeline > li > span').allTextContents();
      await page.reload();
      await expect(page.getByText('Loading task…')).toHaveCount(0);
      const replayedIds = await page.locator('.timeline > li > span').allTextContents();
      expect(new Set(replayedIds).size).toBe(replayedIds.length);
      expect(replayedIds.length).toBeGreaterThanOrEqual(eventIdsBefore.length);

      await expect.poll(async () => {
        const status = await page.locator('.status strong').textContent();
        return status === 'FAILED' ? status : status === 'DONE' ? status : 'RUNNING';
      }, { timeout: 120_000 }).toBe('DONE');
      await page.reload();
      await expect(page.getByText('Loading task…')).toHaveCount(0);
      const eventIds = await page.locator('.timeline > li > span').allTextContents();
      expect(new Set(eventIds).size).toBe(eventIds.length);
      expect(eventIds.length).toBeGreaterThanOrEqual(eventIdsBefore.length);
      await page.getByRole('button', { name: 'Start' }).press('Enter');
      const visibleFailure = page.getByRole('alert');
      await expect(visibleFailure).toBeVisible();
      const failureText = await visibleFailure.textContent();
      await expect(page.getByRole('heading', { name: 'Verification results' })).toBeVisible();
      await expect(page.getByText(/passed|exit_code/i)).toBeVisible();
      await expect(page.getByRole('heading', { name: 'Artifacts' })).toBeVisible();
      await expect(page.getByRole('link', { name: 'Download' }).first()).toBeVisible();
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
