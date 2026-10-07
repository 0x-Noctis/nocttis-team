// M5-009: skenario wajib yang belum tercakup spec lain, terhadap backend nyata + fake provider per-task:
// timeout dan gangguan provider, error permanen, test gagal, restart (SIGKILL), keputusan manusia lewat UI, dan alur
// Lead -> approval -> eksekusi paralel backend/frontend. Skenario lain berada di parallel/lead-dag/vertical-smoke
// dan semuanya dipetakan di docs/test-matrix.md. Test terakhir menjaga agar tag wajib tidak hilang.
const { expect, test } = require('../../web/node_modules/@playwright/test');
const { randomUUID } = require('node:crypto');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { FIXTURE_FILES, objective } = require('../scenarios/parallel/scenarios.cjs');
const { REQUIRED } = require('../scenarios/matrix.cjs');
const { clearLeftoverModel, retrying } = require('./support.cjs');

const POSTGRES = 'noctis-agent-3-e2e-postgres';
const key = () => ({ 'Idempotency-Key': randomUUID() });
const suffix = Date.now().toString(36);
const TERMINAL = new Set(['DONE', 'NEEDS_HUMAN', 'CANCELLED', 'FAILED']);

let repository;
let projectId;
let baseCommit;

function sql(statement) {
  const result = spawnSync(
    'docker',
    ['exec', '-i', POSTGRES, 'psql', '-U', 'ai_team', '-d', 'ai_team', '-v', 'ON_ERROR_STOP=1', '-At', '-F', '|', '-c', statement],
    { encoding: 'utf8' }
  );
  if (result.status !== 0) throw new Error(`psql failed: ${result.stderr}`);
  return result.stdout.trim();
}

function git(cwd, ...args) {
  const result = spawnSync('git', ['-C', cwd, ...args], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(`git ${args[0]} failed: ${result.stderr}`);
  return result.stdout.trim();
}

function makeRepository(prefix, files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), prefix));
  spawnSync('git', ['init', '--initial-branch=main', dir]);
  git(dir, 'config', 'user.name', 'Noctis E2E');
  git(dir, 'config', 'user.email', 'noctis@example.invalid');
  for (const [file, content] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
    fs.writeFileSync(path.join(dir, file), content);
  }
  git(dir, 'add', '.');
  git(dir, 'commit', '-m', 'fixture');
  return dir;
}

async function until(check, what, timeout = 120_000) {
  const deadline = Date.now() + timeout;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`timeout waiting for ${typeof what === 'function' ? what() : what}`);
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
}

const rows = (statement) => sql(statement).split('\n').filter(Boolean).map((line) => line.split('|'));
const taskStatuses = (run) => Object.fromEntries(rows(`SELECT id,status FROM tasks WHERE project_run_id='${run}'`));
const attempts = (run) =>
  rows(
    `SELECT a.task_id,a.status,coalesce(a.error_code,''),extract(epoch from a.started_at),coalesce(extract(epoch from a.finished_at),0) FROM agent_runs a JOIN tasks t ON t.id=a.task_id WHERE t.project_run_id='${run}' ORDER BY a.started_at`
  ).map(([task, status, error, started, finished]) => ({ task, status, error, started: Number(started), finished: Number(finished) }));
const integrationSubjects = (repo, run) => {
  const branch = `noctis-integration-${run}`;
  if (!git(repo, 'branch', '--list', branch)) return [];
  return git(repo, 'log', '--format=%s', `${baseCommit}..${branch}`).split('\n').filter(Boolean);
};
const providerRequests = async () => (await fetch('http://127.0.0.1:7411/stats')).json();
const baseIntact = (repo) => {
  expect(git(repo, 'rev-parse', 'main')).toBe(baseCommit);
  expect(git(repo, 'status', '--porcelain')).toBe('');
};

/** Run RUNNING + task READY lewat SQL dalam satu transaksi. Mengembalikan id task lengkap dan run. */
function createRun(name, tasks, budget = 100000) {
  const run = randomUUID();
  const statements = [`INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ('${run}','${projectId}','${name}','RUNNING',${budget})`];
  const ids = {};
  for (const task of tasks) {
    const { id, file, verify, maxAttempts = 1, ...behaviour } = task;
    ids[id] = `${name}-${id}-${suffix}`;
    statements.push(
      `INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts,max_tool_calls,timeout_seconds) VALUES ('${ids[id]}','${run}','worker','${name} ${id}','${objective(ids[id], { file, ...behaviour })}','READY','${JSON.stringify([file])}','["works"]','${JSON.stringify([verify ?? `cat ${file}`])}',2000,1000,${maxAttempts},10,120)`
    );
  }
  return { run, ids, statements: statements.join(';\n') };
}

/** Daftarkan provider + model e2e-model (tools terverifikasi); mengembalikan fungsi pembersih. */
async function withModel(request, timeoutSeconds = 30) {
  await clearLeftoverModel(request, key);
  const providerId = `matrix-${randomUUID().slice(0, 8)}`;
  const provider = await request.post('/api/v1/providers', {
    headers: key(),
    data: { id: providerId, base_url: 'http://127.0.0.1:7411/v1', api_key_env: 'PRIMARY_API_KEY', request_timeout_seconds: timeoutSeconds }
  });
  expect(provider.ok()).toBeTruthy();
  const model = await request.post(`/api/v1/providers/${providerId}/models`, {
    headers: key(),
    data: {
      id: 'e2e-model', provider_id: providerId, remote_name: 'fixture-model', class: 'coding',
      context_window: 65536, max_output_tokens: 4096,
      claimed_capabilities: { chat: true, streaming: true, tools: true, parallel_tools: false }
    }
  });
  expect(model.ok()).toBeTruthy();
  const probe = await request.post('/api/v1/models/e2e-model/probes/tools', { headers: key() });
  expect((await probe.json()).result.verified).toBe('supported');
  return () => retrying(() => request.delete(`/api/v1/providers/${providerId}`, { headers: key() }));
}

async function restartBackend(env = {}) {
  const response = await fetch('http://127.0.0.1:7412/restart-backend', { method: 'POST', body: JSON.stringify({ env }) });
  expect(response.status).toBe(200);
}

test.describe.configure({ mode: 'serial' });

test.beforeAll(async ({ request }) => {
  repository = makeRepository('noctis-matrix-e2e-', FIXTURE_FILES);
  baseCommit = git(repository, 'rev-parse', 'main');
  projectId = randomUUID();
  const created = await request.post('/api/v1/projects', { headers: key(), data: { id: projectId, name: 'Matrix E2E', repository_path: repository } });
  expect(created.status()).toBe(201);
  // Scheduler melayani semua run; jeda run RUNNING milik spec lain supaya task sisa mereka tidak ikut berjalan.
  sql(`UPDATE project_runs SET status='PAUSED' WHERE status='RUNNING' AND project_id<>'${projectId}'`);
});

test.afterAll(() => fs.rmSync(repository, { recursive: true, force: true }));

test.describe('provider reliability', () => {
  test('[matrix:provider-timeout-retry] a provider timeout is retried safely and the task completes once', async ({ request }) => {
    const dispose = await withModel(request, 2); // timeout request 2 dtk
    try {
      const { run, ids, statements } = createRun('timeout', [{ id: 't', file: 'a.txt', to: 'timeout-done', slowfirst: 3500 }]);
      sql(statements);
      await until(() => TERMINAL.has(taskStatuses(run)[ids.t]), 'task terminal');
      expect(taskStatuses(run)[ids.t]).toBe('DONE');
      // Request pertama melewati timeout; retry membuat satu attempt tetap selesai: 1 attempt, bukan 2.
      expect(attempts(run).map((a) => [a.status, a.error])).toEqual([['completed', '']]);
      // patch (timeout) + patch (retry) + ringkasan + review
      expect((await providerRequests())[ids.t]).toBe(4);
      expect(integrationSubjects(repository, run)).toEqual([`integrate ${ids.t}`]);
      baseIntact(repository);
    } finally {
      await dispose();
    }
  });

  test('[matrix:provider-transient-retry] transient 503 responses are retried with backoff', async ({ request }) => {
    const dispose = await withModel(request);
    try {
      const { run, ids, statements } = createRun('transient', [{ id: 't', file: 'b.txt', to: 'transient-done', fail: 503, failn: 2 }]);
      sql(statements);
      await until(() => TERMINAL.has(taskStatuses(run)[ids.t]), 'task terminal');
      expect(taskStatuses(run)[ids.t]).toBe('DONE');
      expect(attempts(run)).toHaveLength(1);
      expect((await providerRequests())[ids.t]).toBe(5); // 2 x 503 + patch + ringkasan + review
      baseIntact(repository);
    } finally {
      await dispose();
    }
  });

  test('[matrix:provider-permanent-error-no-retry] an authentication error is not retried and nothing is integrated', async ({ request }) => {
    const dispose = await withModel(request);
    try {
      const { run, ids, statements } = createRun('permanent', [
        { id: 'bad', file: 'c.txt', to: 'never', fail: 401, failn: 1 },
        { id: 'good', file: 'd.txt', to: 'good-done' }
      ]);
      sql(statements);
      await until(() => taskStatuses(run)[ids.good] === 'DONE' && attempts(run).some((a) => a.task === ids.bad && a.finished > 0), 'both finished');
      const bad = attempts(run).filter((a) => a.task === ids.bad);
      expect(bad).toHaveLength(1);
      expect(bad[0].status).toBe('failed');
      expect(bad[0].error).toMatch(/^(worker|orchestrator)\./);
      expect((await providerRequests())[ids.bad]).toBe(1); // satu request, tanpa pengulangan
      expect(taskStatuses(run)[ids.bad]).not.toBe('DONE');
      // Kegagalan satu task tidak menghentikan task lain, dan hasilnya tidak ikut terintegrasi.
      expect(integrationSubjects(repository, run)).toEqual([`integrate ${ids.good}`]);
      baseIntact(repository);
    } finally {
      await dispose();
    }
  });
});

test.describe('verification and human decisions', () => {
  test('[matrix:test-failure-blocks-integration] a failing verification prevents integration and spares the base', async ({ request }) => {
    const dispose = await withModel(request);
    try {
      const { run, ids, statements } = createRun('verifyfail', [
        { id: 'bad', file: 'a.txt', to: 'bad-done', verify: 'grep -q impossible a.txt' },
        { id: 'good', file: 'b.txt', to: 'good-done' }
      ]);
      sql(statements);
      await until(() => taskStatuses(run)[ids.good] === 'DONE' && attempts(run).some((a) => a.task === ids.bad && a.finished > 0), 'both finished');
      const bad = attempts(run).find((a) => a.task === ids.bad);
      expect([bad.status, bad.error]).toEqual(['failed', 'verification.failed']);
      expect(taskStatuses(run)[ids.bad]).not.toBe('DONE');
      expect(integrationSubjects(repository, run)).toEqual([`integrate ${ids.good}`]);
      baseIntact(repository);
    } finally {
      await dispose();
    }
  });

  test('[matrix:approval-human-decision] a person retries a task from the Approvals page with identity and confirmation', async ({ page }) => {
    // Task NEEDS_HUMAN pada run PAUSED: setelah retry ia READY tetapi scheduler tidak menyentuhnya.
    const run = randomUUID();
    const task = `human-${suffix}`;
    sql(
      [
        `INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ('${run}','${projectId}','human decision','PAUSED',100000)`,
        `INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts) VALUES ('${task}','${run}','worker','Needs a person','o','NEEDS_HUMAN','["a.txt"]','["ok"]','["true"]',1000,1000,3)`
      ].join(';\n')
    );
    await page.goto('/approvals');
    await expect(page.getByText(/Loading/)).toHaveCount(0);
    const card = page.locator('article', { hasText: task });
    await expect(card).toBeVisible();

    // Tanpa identitas, aksi dinonaktifkan dan alasannya terbaca.
    await expect(card.getByRole('button', { name: /Retry task/ })).toBeDisabled();
    await page.getByLabel('Your name or ID').fill('e2e-reviewer');
    await card.getByRole('button', { name: /Retry task/ }).click();
    // Dua langkah: konfirmasi eksplisit dengan alasan wajib.
    await card.getByRole('button', { name: 'Yes, retry' }).click();
    await expect(card.getByRole('alert')).toContainText(/required/i);
    await card.getByLabel('Why is retrying safe?').fill('Checked the logs; no side effects remain.');
    await card.getByRole('button', { name: 'Yes, retry' }).click();
    await expect(page.getByRole('status').filter({ hasText: 'sent back to READY as e2e-reviewer' })).toBeVisible();

    expect(sql(`SELECT status FROM tasks WHERE id='${task}'`)).toBe('READY');
    // Jejak audit: siapa yang memutuskan tercatat.
    expect(sql(`SELECT count(*) FROM events WHERE task_id='${task}' AND actor_id='e2e-reviewer'`)).not.toBe('0');
  });
});

test.describe('process restart', () => {
  test('[matrix:restart-recovery] a killed server recovers the run and integrates exactly once', async ({ request }) => {
    const dispose = await withModel(request);
    try {
      const { run, ids, statements } = createRun('restart', [{ id: 't', file: 'a.txt', to: 'restart-done', delay: 6000, maxAttempts: 2 }]);
      sql(statements);
      await until(() => attempts(run).some((a) => a.status === 'running' || a.status === 'assigned'), 'attempt running');
      // Matikan server (SIGKILL) saat worker menunggu model, lalu nyalakan lagi dengan jendela stale pendek.
      await restartBackend({ NOCTIS__SCHEDULER__HEARTBEAT_SECONDS: '2', NOCTIS__SCHEDULER__STALE_AFTER_SECONDS: '8' });
      await until(() => TERMINAL.has(taskStatuses(run)[ids.t]), 'task terminal after restart', 150_000);

      expect(taskStatuses(run)[ids.t]).toBe('DONE');
      const history = attempts(run);
      expect(history).toHaveLength(2); // attempt yang terputus + attempt pemulihan
      expect(history[0].status).toBe('failed');
      expect(history[1].status).toBe('completed');
      // Tanpa efek ganda: satu commit integrasi dan satu baris integrasi tercatat.
      expect(integrationSubjects(repository, run)).toEqual([`integrate ${ids.t}`]);
      expect(sql(`SELECT count(*) FROM integration_operations WHERE task_id='${ids.t}'`)).toBe('1');
      baseIntact(repository);
    } finally {
      await restartBackend({}); // kembalikan konfigurasi bawaan untuk spec berikutnya
      await dispose();
    }
  });
});

test.describe('lead to execution', () => {
  test('[matrix:backend-frontend-parallel] an approved Lead plan runs backend and frontend in parallel and integrates all four tasks', async ({ page, request }) => {
    // Verifikasi memakai `npm test` (hasil discovery) sehingga butuh image Node lokal; dilewati bila tidak ada.
    const image = 'node:22-bookworm-slim';
    if (spawnSync('docker', ['image', 'inspect', image], { stdio: 'ignore' }).status !== 0) {
      test.skip(true, `image ${image} tidak ada secara lokal (test tidak menarik image lewat jaringan)`);
    }
    const repo = makeRepository('noctis-lead-exec-', {
      'package.json': JSON.stringify({ name: 'lead-exec', version: '1.0.0', scripts: { test: 'node -e ""' } }),
      'README.md': 'fixture\n'
    });
    const base = git(repo, 'rev-parse', 'main');
    const project = randomUUID();
    const dispose = await withModel(request);
    try {
      const created = await request.post('/api/v1/projects', { headers: key(), data: { id: project, name: 'Lead exec', repository_path: repo } });
      expect(created.status()).toBe(201);
      await restartBackend({ NOCTIS_RUNNER_IMAGE: image });

      await page.goto(`/projects/${project}`);
      await expect(page.getByText('Loading project…')).toHaveCount(0);
      await page.getByLabel('Objective').fill('[scenario:execute] Add product search');
      await page.getByLabel('Acceptance criteria').fill('Search works');
      await page.getByLabel('Token budget').fill('100000');
      const run = await page.getByLabel('Run ID').inputValue();
      await page.getByRole('button', { name: 'Create run' }).click();
      await expect(page).toHaveURL(new RegExp(`/runs/${run}$`));
      await expect(page.getByText('Loading run…')).toHaveCount(0);

      await page.getByRole('button', { name: 'Ask Lead to plan' }).click();
      await expect(page.getByRole('heading', { name: /Proposed plan · 4 tasks/ })).toBeVisible({ timeout: 30_000 });
      // Persetujuan manusia lewat keyboard.
      await page.getByLabel('Actor ID').fill('e2e-human');
      await page.getByRole('checkbox').focus();
      await page.keyboard.press('Space');
      await page.getByRole('button', { name: 'Approve plan' }).focus();
      await page.keyboard.press('Enter');
      await expect(page.getByRole('status').filter({ hasText: 'Plan approved' })).toBeVisible();

      try {
        await until(() => {
          const statuses = Object.values(taskStatuses(run));
          return statuses.length === 4 && statuses.every((status) => TERMINAL.has(status));
        }, () => `lead tasks terminal: ${JSON.stringify(taskStatuses(run))} ${JSON.stringify(attempts(run))}`, 120_000);
      } catch (error) {
        // Diagnosis: apa yang dilihat provider dan event apa yang tercatat untuk task yang gagal.
        const events = rows(`SELECT task_id,event_type,left(payload::text,200) FROM events WHERE project_run_id='${run}' ORDER BY id DESC LIMIT 12`);
        throw new Error(`${error.message}\nprovider=${JSON.stringify(await providerRequests())}\nevents=${JSON.stringify(events)}`);
      }

      const statuses = taskStatuses(run);
      expect(Object.values(statuses)).toEqual(['DONE', 'DONE', 'DONE', 'DONE']);
      const byName = (name) => attempts(run).find((a) => a.task.endsWith(`-${name}`));
      const [contract, backend, frontend, final] = ['contract', 'backend', 'frontend', 'search-test'].map(byName);
      // Urutan dependency dan paralelisme: backend dan frontend berjalan bersamaan setelah contract, test paling akhir.
      expect(backend.started).toBeGreaterThanOrEqual(contract.finished);
      expect(frontend.started).toBeGreaterThanOrEqual(contract.finished);
      expect(backend.started).toBeLessThan(frontend.finished);
      expect(frontend.started).toBeLessThan(backend.finished);
      expect(final.started).toBeGreaterThanOrEqual(Math.max(backend.finished, frontend.finished));
      expect(attempts(run)).toHaveLength(4);
      // Semua patch masuk cabang integrasi (empat commit) dan cabang dasar tidak tersentuh.
      const log = git(repo, 'log', '--format=%s', `${base}..noctis-integration-${run}`).split('\n').filter(Boolean);
      expect(log).toHaveLength(4);
      for (const file of ['docs/contract.md', 'src/backend.js', 'src/frontend.js', 'test/search.test.js']) {
        expect(git(repo, 'show', `noctis-integration-${run}:${file}`)).toMatch(/-done$/);
      }
      expect(git(repo, 'rev-parse', 'main')).toBe(base);
      expect(git(repo, 'status', '--porcelain')).toBe('');
    } finally {
      await restartBackend({});
      await dispose();
      fs.rmSync(repo, { recursive: true, force: true });
    }
  });
});

test('[matrix:coverage-check] every required scenario still has a tagged E2E test', async () => {
  const specs = fs.readdirSync(__dirname).filter((name) => name.endsWith('.spec.cjs'));
  const text = specs.map((name) => fs.readFileSync(path.join(__dirname, name), 'utf8')).join('\n');
  // Hanya judul test yang dihitung: tag harus berada di dalam string judul `test('[matrix:...] ...')`.
  const tagged = new Set([...text.matchAll(/test(?:\.skip)?\(\s*['"`]\[matrix:([a-z-]+)\]/g)].map((match) => match[1]));
  const missing = REQUIRED.filter((scenario) => !tagged.has(scenario.id)).map((scenario) => `${scenario.id} (${scenario.what})`);
  expect(missing, `skenario wajib tanpa test bertag:\n${missing.join('\n')}`).toEqual([]);
});
