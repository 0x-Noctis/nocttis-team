// M5-008: halaman Operations dan Settings terhadap backend nyata: kesehatan, penanda estimasi, kesiapan provider,
// komponen degraded, dan jaminan bahwa secret/path server tidak tampil.
const { expect, test } = require('../../web/node_modules/@playwright/test');
const { randomUUID } = require('node:crypto');
const { spawnSync } = require('node:child_process');

const POSTGRES = 'noctis-agent-3-e2e-postgres';
const key = () => ({ 'Idempotency-Key': randomUUID() });
const suffix = Date.now().toString(36);
const providerId = `ops-provider-${suffix}`;
const modelId = `ops-model-${suffix}`;
const project = randomUUID();
const run = randomUUID();
const attempt = randomUUID();
const staleAttempt = randomUUID();

function sql(statement) {
  const result = spawnSync(
    'docker',
    ['exec', '-i', POSTGRES, 'psql', '-U', 'ai_team', '-d', 'ai_team', '-v', 'ON_ERROR_STOP=1', '-c', statement],
    { encoding: 'utf8' }
  );
  if (result.status !== 0) throw new Error(`psql failed: ${result.stderr}`);
}

test.describe.configure({ mode: 'serial' });

test.beforeAll(async ({ request }) => {
  const provider = await request.post('/api/v1/providers', {
    headers: key(),
    data: { id: providerId, base_url: 'http://127.0.0.1:7411/v1', api_key_env: 'PRIMARY_API_KEY', request_timeout_seconds: 10 }
  });
  expect(provider.ok()).toBeTruthy();
  // Model tanpa probe: kemampuan tools belum terverifikasi => provider harus tampil "Needs attention".
  const model = await request.post(`/api/v1/providers/${providerId}/models`, {
    headers: key(),
    data: {
      id: modelId, provider_id: providerId, remote_name: 'fixture-model', class: 'coding',
      context_window: 8192, max_output_tokens: 2048,
      claimed_capabilities: { chat: true, streaming: false, tools: true, parallel_tools: false }
    }
  });
  expect(model.ok()).toBeTruthy();
  const task = (id) =>
    `INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts) VALUES ('${id}','${run}','worker','Ops task','o','DONE','["a.txt"]','["ok"]','["true"]',1000,1000,2)`;
  sql(
    [
      `INSERT INTO projects (id,name,repository_path) VALUES ('${project}','Ops project','/tmp/ops-${suffix}')`,
      `INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ('${run}','${project}','ops','PAUSED',100000)`,
      task(`ops-done-${suffix}`),
      task(`ops-stale-${suffix}`),
      `INSERT INTO agent_runs (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,finished_at,retain_until) VALUES ('${attempt}','ops-done-${suffix}','worker','${providerId}','${modelId}',1,'completed','b1','${'0'.repeat(40)}',now(),now(),now())`,
      // 1.000 token dari provider + 500 token estimasi
      `INSERT INTO model_usage (agent_run_id,input_tokens,output_tokens,latency_ms,estimated) VALUES ('${attempt}',600,400,1,false)`,
      `INSERT INTO model_usage (agent_run_id,input_tokens,output_tokens,latency_ms,estimated) VALUES ('${attempt}',300,200,1,true)`
    ].join(';\n')
  );
});

test.afterAll(async ({ request }) => {
  // Hanya baris milik spec ini (agent_runs menahan provider lewat foreign key).
  sql(`DELETE FROM model_usage WHERE agent_run_id IN ('${attempt}','${staleAttempt}'); DELETE FROM agent_runs WHERE id IN ('${attempt}','${staleAttempt}')`);
  await request.delete(`/api/v1/providers/${providerId}`, { headers: key() });
});

test('operations: health, estimated usage, and provider attention are explicit and secret-free', async ({ page }) => {
  await page.goto('/operations');
  await expect(page.getByText('Loading operations…')).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Operations', level: 1 })).toBeVisible();

  // Kesehatan: kata + penjelasan, bukan hanya warna.
  const health = page.getByRole('region', { name: 'Service health' });
  await expect(health.getByRole('status')).toContainText(/Ready/);
  for (const name of ['Database', 'Migrations', 'Workers', 'Model providers']) {
    await expect(health.getByRole('heading', { name, level: 3 })).toBeVisible();
  }
  await expect(health.getByRole('heading', { name: 'Database' }).locator('..')).toContainText('OK');

  // Usage: bagian estimasi diberi label eksplisit.
  const usage = page.getByRole('region', { name: 'Usage and cost' });
  await expect(usage).toContainText('Estimated:');
  await expect(usage).toContainText('approximate');
  await expect(usage).toContainText('Not tracked');

  // Provider: kunci terkonfigurasi (hanya status), tetapi tools belum terverifikasi => perlu perhatian.
  const card = page.getByRole('region', { name: 'Provider readiness' }).getByRole('listitem').filter({ hasText: providerId }).first();
  await expect(card).toContainText('Needs attention');
  await expect(card).toContainText('configured on server');
  await expect(card).toContainText('No model has verified tool support');
  await expect(card).toContainText('127.0.0.1:7411');

  // Tidak ada secret, nama env var, query URL, atau path server di halaman.
  const text = await page.locator('main').innerText();
  for (const forbidden of [process.env.NOCTIS_E2E_API_KEY, 'PRIMARY_API_KEY', '/home/', '/tmp/', 'api/v1/chat']) {
    expect(text, `bocor: ${forbidden}`).not.toContain(forbidden);
  }
});

test('operations: a stale worker marks Workers as degraded with an explanation', async ({ page }) => {
  sql(
    `INSERT INTO agent_runs (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,retain_until) VALUES ('${staleAttempt}','ops-stale-${suffix}','worker','${providerId}','${modelId}',1,'running','b2','${'0'.repeat(40)}',now() - interval '1 hour',now() + interval '1 hour')`
  );
  await page.goto('/operations');
  await expect(page.getByText('Loading operations…')).toHaveCount(0);
  const workers = page.getByRole('heading', { name: 'Workers', level: 3 }).locator('..');
  await expect(workers).toContainText('Degraded');
  await expect(workers).toContainText('stopped sending heartbeats');
  await expect(page.getByRole('region', { name: 'Queue and workers' })).toContainText('1 stale attempt');
  // Degraded tidak berarti tidak siap: layanan tetap "Ready, with warnings".
  await expect(page.getByRole('region', { name: 'Service health' }).getByRole('status')).toContainText('Ready, with warnings');
});

test('settings: shows effective non-secret configuration only', async ({ page }) => {
  await page.goto('/settings');
  await expect(page.getByText('Loading settings…')).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Settings', level: 1 })).toBeVisible();
  const scheduler = page.getByRole('region', { name: 'Scheduler' });
  await expect(scheduler.locator('div', { hasText: 'Max parallel agents' }).first()).toContainText('4');
  await expect(page.getByRole('region', { name: 'Runner' })).toContainText('No'); // network_enabled
  const text = await page.locator('main').innerText();
  for (const forbidden of [process.env.NOCTIS_E2E_API_KEY, 'ai_team_dev', 'postgres://', '/home/', '/tmp/']) {
    expect(text, `bocor: ${forbidden}`).not.toContain(forbidden);
  }
  // Catatan keamanan terlihat.
  await expect(page.getByRole('note')).toContainText('never shown');
});
