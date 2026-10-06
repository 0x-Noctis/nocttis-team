// M3-012: skenario Lead/DAG lewat browser nyata + backend produksi + fake provider (tests/scenarios/lead).
//
// Penting untuk determinisme: model `e2e-model` hanya didaftarkan selama panggilan Lead lalu provider
// dihapus. Tanpa model terdaftar scheduler menunggu tenang, jadi task hasil approval tidak benar-benar
// dieksekusi dan board tetap pada keadaan awal (root Queued, sisanya Blocked).
const { expect, test } = require('../../web/node_modules/@playwright/test');
const { randomUUID } = require('node:crypto');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const key = () => ({ 'Idempotency-Key': randomUUID() });
let repository;
let projectId;

function git(...args) {
  const result = spawnSync('git', ['-C', repository, ...args], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(`git ${args[0]} failed`);
}

test.beforeAll(async ({ request }) => {
  // Repository kecil dengan script test supaya discovery menghasilkan `npm test`.
  repository = fs.mkdtempSync(path.join(os.tmpdir(), 'noctis-lead-e2e-'));
  fs.mkdirSync(path.join(repository, 'src'));
  fs.writeFileSync(path.join(repository, 'package.json'), JSON.stringify({ name: 'lead-fixture', scripts: { test: 'node -e ""' } }));
  fs.writeFileSync(path.join(repository, 'src/backend.js'), 'module.exports = {};\n');
  spawnSync('git', ['init', '--initial-branch=main', repository]);
  git('config', 'user.name', 'Noctis E2E');
  git('config', 'user.email', 'noctis@example.invalid');
  git('add', '.');
  git('commit', '-m', 'fixture');
  projectId = randomUUID();
  const created = await request.post('/api/v1/projects', { headers: key(), data: { id: projectId, name: 'Lead E2E', repository_path: repository } });
  expect(created.status()).toBe(201);
});

test.afterAll(() => fs.rmSync(repository, { recursive: true, force: true }));

/** Daftarkan provider+model default hanya selama `work`, lalu hapus supaya scheduler tidak menjalankan task. */
async function withLeadModel(request, work) {
  const providerId = `lead-${randomUUID().slice(0, 8)}`;
  const provider = await request.post('/api/v1/providers', {
    headers: key(),
    data: { id: providerId, base_url: 'http://127.0.0.1:7411/v1', api_key_env: 'PRIMARY_API_KEY', request_timeout_seconds: 10 }
  });
  expect(provider.ok()).toBeTruthy();
  try {
    const model = await request.post(`/api/v1/providers/${providerId}/models`, {
      headers: key(),
      data: {
        id: 'e2e-model', provider_id: providerId, remote_name: 'fixture-model', class: 'reasoning',
        context_window: 32000, max_output_tokens: 4000,
        claimed_capabilities: { chat: true, streaming: false, tools: false, parallel_tools: false }
      }
    });
    expect(model.ok()).toBeTruthy();
    await work();
  } finally {
    await request.delete(`/api/v1/providers/${providerId}`, { headers: key() });
  }
}

/** Buat run lewat UI di halaman project; mengembalikan run ID. */
async function startRun(page, objective, budget = 100000) {
  await page.goto(`/projects/${projectId}`);
  await expect(page.getByText('Loading project…')).toHaveCount(0);
  await page.getByLabel('Objective').fill(objective);
  await page.getByLabel('Acceptance criteria').fill('Search works\nTests pass');
  await page.getByLabel('Token budget').fill(String(budget));
  const run = await page.getByLabel('Run ID').inputValue();
  await page.getByRole('button', { name: 'Create run' }).click();
  await expect(page).toHaveURL(new RegExp(`/runs/${run}$`));
  await expect(page.getByText('Loading run…')).toHaveCount(0);
  return run;
}

const plansOf = async (request, run) => (await (await request.get(`/api/v1/runs/${run}/plans`)).json()).items;
const tasksOf = async (request, run) => (await (await request.get(`/api/v1/tasks?project_run_id=${run}`)).json()).items;

async function approve(page) {
  await page.getByLabel('Actor ID').fill('e2e-human');
  // Jalur keyboard: centang konfirmasi dengan Space, lalu Enter pada tombol Approve.
  await page.getByRole('checkbox').focus();
  await page.keyboard.press('Space');
  await page.getByRole('button', { name: 'Approve plan' }).focus();
  await page.keyboard.press('Enter');
}

test.describe('Lead and task DAG', () => {
  test('valid plan: DAG order, risk flag, human approval, blocked tasks', async ({ page, request }) => {
    const run = await startRun(page, '[scenario:valid] Add product search');
    await expect(page.getByText('No plan yet')).toBeVisible();
    await expect(page.getByLabel('Run status: Planning')).toBeVisible();

    await withLeadModel(request, async () => {
      await page.getByRole('button', { name: 'Ask Lead to plan' }).click();
      await expect(page.getByRole('heading', { name: /Proposed plan · 4 tasks/ })).toBeVisible({ timeout: 20_000 });
    });
    await expect(page.getByRole('status').filter({ hasText: 'Lead proposed a plan' })).toBeVisible();
    await expect(page.getByLabel('Run status: Awaiting approval')).toBeVisible();
    await expect(page.getByText('backend and frontend share the products response shape')).toBeVisible();
    // Urutan dependency: contract -> backend/frontend -> search-test.
    await expect(page.getByRole('heading', { name: 'Step 1 · no dependencies' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Step 2' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Step 3' })).toBeVisible();
    // Reservasi = 4 x (2.000 + 1.000) dari 100.000 tersisa; tidak ada peringatan.
    await expect(page.getByText('12,000')).toBeVisible();
    await expect(page.getByText('Approval will be refused')).toHaveCount(0);
    // Belum ada task sebelum manusia menyetujui.
    expect(await tasksOf(request, run)).toHaveLength(0);

    await approve(page);
    await expect(page.getByRole('status').filter({ hasText: 'Plan approved' })).toBeVisible();
    await expect(page.getByLabel('Run status: Running')).toBeVisible();
    expect(await tasksOf(request, run)).toHaveLength(4);
    // Root menunggu giliran; tiga task lain terblokir dengan alasan yang terbaca.
    await expect(page.getByRole('heading', { name: /Queued/ })).toContainText('(1)');
    await expect(page.getByRole('heading', { name: /Blocked/ })).toContainText('(3)');
    // Alasan yang sama juga tampil di panel antrean dashboard paralel; periksa kartu di board saja.
    const board = page.locator('section[aria-labelledby="board-title"]');
    await expect(board.getByText(/Waiting for [0-9a-f]{8}-contract \(/).first()).toBeVisible();
    await expect(board.getByText(/Waiting for [0-9a-f]{8}-backend \(.*\), [0-9a-f]{8}-frontend \(/)).toBeVisible();
  });

  test('rejected plan creates no tasks and Lead can re-plan as version 2', async ({ page, request }) => {
    const run = await startRun(page, '[scenario:valid] Reject then replan');
    await withLeadModel(request, async () => {
      await page.getByRole('button', { name: 'Ask Lead to plan' }).click();
      await expect(page.getByRole('heading', { name: /Proposed plan · 4 tasks/ })).toBeVisible({ timeout: 20_000 });

      // Reject tanpa alasan ditolak di UI; dengan alasan diterima.
      await page.getByLabel('Actor ID').fill('e2e-human');
      await page.getByRole('button', { name: 'Reject plan' }).click();
      await expect(page.getByRole('alert').filter({ hasText: 'A reason is required' })).toBeVisible();
      await page.getByLabel(/Reason/).fill('Scope too wide');
      await page.getByRole('button', { name: 'Reject plan' }).click();
      await expect(page.getByRole('status').filter({ hasText: 'Plan rejected' })).toBeVisible();
      await expect(page.getByText('This plan is already rejected')).toBeVisible();
      expect(await tasksOf(request, run)).toHaveLength(0);
      expect((await plansOf(request, run)).map((plan) => plan.status)).toEqual(['REJECTED']);

      await page.getByRole('button', { name: 'Ask Lead for a new plan' }).click();
      await expect(page.getByText(/version 2/)).toBeVisible({ timeout: 20_000 });
    });
    expect((await plansOf(request, run)).map((plan) => [plan.version, plan.status])).toEqual([[2, 'PROPOSED'], [1, 'REJECTED']]);
    await expect(page.getByLabel('Run status: Awaiting approval')).toBeVisible();
  });

  test('cyclic plan is refused with a visible message and nothing is stored', async ({ page, request }) => {
    const run = await startRun(page, '[scenario:cycle] Circular dependencies');
    await withLeadModel(request, async () => {
      await page.getByRole('button', { name: 'Ask Lead to plan' }).click();
      await expect(page.getByRole('alert').filter({ hasText: 'lead plan is invalid' })).toBeVisible({ timeout: 20_000 });
    });
    await expect(page.getByText('No plan yet')).toBeVisible();
    await expect(page.getByLabel('Run status: Planning')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Ask Lead to plan' })).toBeEnabled();
    expect(await plansOf(request, run)).toHaveLength(0);
  });

  test('over-budget plan is refused by the Lead budget guard', async ({ page, request }) => {
    const run = await startRun(page, '[scenario:overbudget] Too much work');
    await withLeadModel(request, async () => {
      await page.getByRole('button', { name: 'Ask Lead to plan' }).click();
      await expect(page.getByRole('alert').filter({ hasText: 'lead plan exceeds run token budget' })).toBeVisible({ timeout: 20_000 });
    });
    await expect(page.getByText('No plan yet')).toBeVisible();
    expect(await plansOf(request, run)).toHaveLength(0);
  });

  test('plan needing more tokens than remain is flagged and approval is refused', async ({ page, request }) => {
    // Lead tidak akan membuat plan seperti ini, jadi dikirim lewat API (jalur manual) dengan budget run 1.000.
    const run = await startRun(page, '[scenario:valid] Tiny budget', 1000);
    const taskId = randomUUID();
    const proposed = await request.post(`/api/v1/runs/${run}/plan`, {
      headers: key(),
      data: {
        id: `plan-${run}`, project_run_id: run, version: 1, risk_flags: [],
        tasks: [{
          id: taskId, project_id: projectId, project_run_id: run, title: 'Too big', role: 'worker', objective: 'Do',
          depends_on: [], allowed_paths: ['src/backend.js'], context_refs: [], acceptance_criteria: ['ok'],
          verification_commands: ['npm test'],
          limits: { max_input_tokens: 2000, max_output_tokens: 1000, max_tool_calls: 5, max_attempts: 1, timeout_seconds: 60 }
        }]
      }
    });
    expect(proposed.status()).toBe(201);
    // Muncul lewat polling tanpa reload; peringatan reservasi tampil sebelum manusia memutuskan.
    await expect(page.getByRole('heading', { name: /Proposed plan · 1 tasks/ })).toBeVisible({ timeout: 15_000 });
    await expect(page.getByText('Approval will be refused')).toBeVisible();

    await approve(page);
    await expect(page.getByRole('alert').filter({ hasText: 'Resource conflict' })).toBeVisible();
    await expect(page.getByLabel('Run status: Awaiting approval')).toBeVisible();
    expect(await tasksOf(request, run)).toHaveLength(0);
    expect((await plansOf(request, run))[0].status).toBe('PROPOSED');
  });
});
