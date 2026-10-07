// M4-010: scheduler paralel + pipeline produksi + Integrator lewat backend nyata, fake provider per-task
// (tests/scenarios/parallel) dan Postgres nyata. Run, task, dan dependency dibuat lewat SQL dalam satu
// transaksi supaya scheduler melihat semuanya bersamaan; hasilnya dibaca dari DB, git, API, dan UI.
//
// Isolasi: run RUNNING milik spec lain dijeda dulu, kalau tidak scheduler (yang melayani semua run) akan
// menjalankan task sisa mereka begitu model terdaftar.
const { expect, test } = require('../../web/node_modules/@playwright/test');
const { randomUUID } = require('node:crypto');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { FIXTURE_FILES, objective } = require('../scenarios/parallel/scenarios.cjs');
const { clearLeftoverModel, retrying } = require('./support.cjs');

const POSTGRES = 'noctis-agent-3-e2e-postgres';
const key = () => ({ 'Idempotency-Key': randomUUID() });
const suffix = Date.now().toString(36);
const TERMINAL = new Set(['DONE', 'NEEDS_HUMAN', 'CANCELLED', 'FAILED']);

let repository;
let projectId;
let providerId;
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

function git(...args) {
  const result = spawnSync('git', ['-C', repository, ...args], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(`git ${args[0]} failed: ${result.stderr}`);
  return result.stdout.trim();
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

/** Buat run RUNNING dan task-nya sekaligus; `tasks` = [{id, file, to?, from?, paths?, verify, priority?, deps?, ...opsi perilaku}]. */
function createRun(name, budget, tasks, extraStatements = () => []) {
  const run = randomUUID();
  const id = (task) => `${name}-${task.id}-${suffix}`;
  const statements = [
    `INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ('${run}','${projectId}','${name}','RUNNING',${budget})`
  ];
  for (const task of tasks) {
    const { id: _id, file, paths, verify, priority = 0, deps, maxAttempts = 1, maxInput = 2000, ...behaviour } = task;
    const text = objective(id(task), { file, ...behaviour });
    statements.push(
      `INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts,max_tool_calls,timeout_seconds,priority) VALUES ('${id(task)}','${run}','worker','${name} ${task.id}','${text}','READY','${JSON.stringify(paths ?? [file])}','["works"]','${JSON.stringify([verify ?? `cat ${file}`])}',${maxInput},1000,${maxAttempts},10,120,${priority})`
    );
  }
  for (const task of tasks) {
    for (const dependency of task.deps ?? []) {
      statements.push(`INSERT INTO task_dependencies (task_id,dependency_id) VALUES ('${id(task)}','${name}-${dependency}-${suffix}')`);
    }
  }
  statements.push(...extraStatements(run));
  return { run, id, statements: statements.join(';\n'), runStatement: statements[0], taskStatements: statements.slice(1).join(';\n') };
}

const taskRows = (run) =>
  Object.fromEntries(
    sql(`SELECT id,status FROM tasks WHERE project_run_id='${run}'`).split('\n').filter(Boolean).map((line) => line.split('|'))
  );

/** Attempt per task dengan jendela waktunya (epoch detik). */
function attempts(run) {
  return sql(
    `SELECT a.task_id,a.status,coalesce(a.error_code,''),extract(epoch from a.started_at),coalesce(extract(epoch from a.finished_at),0) FROM agent_runs a JOIN tasks t ON t.id=a.task_id WHERE t.project_run_id='${run}' ORDER BY a.started_at`
  )
    .split('\n')
    .filter(Boolean)
    .map((line) => {
      const [task, status, error, started, finished] = line.split('|');
      return { task, status, error, started: Number(started), finished: Number(finished) };
    });
}

const integrationBranch = (run) => `noctis-integration-${run}`;
const integrationSubjects = (run) =>
  git('log', '--format=%s', `${baseCommit}..${integrationBranch(run)}`).split('\n').filter(Boolean);

async function settle(run, wanted) {
  await until(() => {
    const rows = taskRows(run);
    return wanted.every((task) => TERMINAL.has(rows[task]));
  }, () => `terminal tasks of run ${run}: ${JSON.stringify(taskRows(run))} ${JSON.stringify(attempts(run))}`);
}

/** Branch dasar tidak boleh berubah apa pun hasil skenarionya. */
function expectBaseIntact() {
  expect(git('rev-parse', 'main')).toBe(baseCommit);
  expect(git('status', '--porcelain')).toBe('');
}

const overlaps = (a, b) => a.started < b.finished && b.started < a.finished;

test.describe.configure({ mode: 'serial' });

test.beforeAll(async ({ request }) => {
  repository = fs.mkdtempSync(path.join(os.tmpdir(), 'noctis-parallel-e2e-'));
  spawnSync('git', ['init', '--initial-branch=main', repository]);
  git('config', 'user.name', 'Noctis E2E');
  git('config', 'user.email', 'noctis@example.invalid');
  for (const [file, content] of Object.entries(FIXTURE_FILES)) {
    fs.mkdirSync(path.dirname(path.join(repository, file)), { recursive: true });
    fs.writeFileSync(path.join(repository, file), content);
  }
  git('add', '.');
  git('commit', '-m', 'parallel fixture');
  baseCommit = git('rev-parse', 'main');

  projectId = randomUUID();
  const created = await request.post('/api/v1/projects', { headers: key(), data: { id: projectId, name: 'Parallel E2E', repository_path: repository } });
  expect(created.status()).toBe(201);
  sql(`UPDATE project_runs SET status='PAUSED' WHERE status='RUNNING' AND project_id<>'${projectId}'`);

  await clearLeftoverModel(request, key);
  providerId = `parallel-${suffix}`;
  const provider = await request.post('/api/v1/providers', {
    headers: key(),
    data: { id: providerId, base_url: 'http://127.0.0.1:7411/v1', api_key_env: 'PRIMARY_API_KEY', request_timeout_seconds: 30 }
  });
  expect(provider.ok()).toBeTruthy();
  const model = await request.post(`/api/v1/providers/${providerId}/models`, {
    headers: key(),
    data: {
      id: 'e2e-model', provider_id: providerId, remote_name: 'fixture-model', class: 'coding',
      context_window: 8192, max_output_tokens: 2048,
      claimed_capabilities: { chat: true, streaming: true, tools: true, parallel_tools: false }
    }
  });
  expect(model.ok()).toBeTruthy();
  const probe = await request.post('/api/v1/models/e2e-model/probes/tools', { headers: key() });
  expect((await probe.json()).result.verified).toBe('supported');
});

test.afterAll(async ({ request }) => {
  await retrying(() => request.delete(`/api/v1/providers/${providerId}`, { headers: key() }));
  fs.rmSync(repository, { recursive: true, force: true });
});

test.describe('parallel scheduler', () => {
  test('[matrix:parallel-independent] two independent tasks run together and both integrate', async ({ request }) => {
    const { run, id, statements } = createRun('indep2', 100000, [
      { id: 'a', file: 'a.txt', to: 'a-done', delay: 3000 },
      { id: 'b', file: 'b.txt', to: 'b-done', delay: 3000 }
    ]);
    sql(statements);
    await settle(run, [id({ id: 'a' }), id({ id: 'b' })]);

    expect(taskRows(run)).toEqual({ [id({ id: 'a' })]: 'DONE', [id({ id: 'b' })]: 'DONE' });
    const rows = attempts(run);
    expect(rows).toHaveLength(2); // satu attempt per task: tidak ada klaim ganda
    expect(overlaps(rows[0], rows[1])).toBe(true);
    expect(integrationSubjects(run).sort()).toEqual([`integrate ${id({ id: 'a' })}`, `integrate ${id({ id: 'b' })}`]);
    expect(git('show', `${integrationBranch(run)}:a.txt`)).toBe('a-done');
    expect(git('show', `${integrationBranch(run)}:b.txt`)).toBe('b-done');
    expectBaseIntact();

    // Snapshot yang sama dengan yang dipakai dashboard.
    const view = await (await request.get(`/api/v1/runs/${run}/scheduler`)).json();
    expect(view.max_slots).toBe(4);
  });

  test('[matrix:parallel-four-workers] four independent tasks fill four slots (visible in the dashboard)', async ({ page }) => {
    const names = ['a', 'b', 'c', 'd'];
    const { run, id, runStatement, taskStatements } = createRun(
      'indep4',
      100000,
      names.map((name) => ({ id: name, file: `${name}.txt`, to: `${name}-done`, delay: 8000 }))
    );
    // Run dibuat dulu supaya halaman bisa dimuat; task menyusul dalam satu transaksi.
    sql(runStatement);
    await page.goto(`/runs/${run}`);
    await expect(page.getByText('Loading run…')).toHaveCount(0);
    sql(taskStatements);
    await expect(page.getByRole('heading', { name: /Workers/ })).toContainText('4 of 4 active', { timeout: 60_000 });
    await settle(run, names.map((name) => id({ id: name })));

    const rows = attempts(run);
    expect(Object.values(taskRows(run))).toEqual(['DONE', 'DONE', 'DONE', 'DONE']);
    expect(rows).toHaveLength(4);
    // Keempat jendela attempt berbagi satu saat yang sama.
    expect(Math.max(...rows.map((row) => row.started))).toBeLessThan(Math.min(...rows.map((row) => row.finished)));
    expect(new Set(integrationSubjects(run)).size).toBe(4);
    for (const name of names) expect(git('show', `${integrationBranch(run)}:${name}.txt`)).toBe(`${name}-done`);
    expectBaseIntact();
  });

  test('[matrix:overlap-held] overlapping scopes are held back until the first task finishes', async () => {
    const { run, id, statements } = createRun('overlap', 100000, [
      { id: 'wide', file: 'src/a.txt', paths: ['src/**'], to: 'wide-done', delay: 3000, priority: 10 },
      { id: 'narrow', file: 'src/b.txt', to: 'narrow-done' }
    ]);
    sql(statements);
    const wide = id({ id: 'wide' });
    const narrow = id({ id: 'narrow' });
    // Selama `wide` belum selesai, `narrow` tidak boleh punya attempt.
    await until(() => {
      // Attempt dibaca SEBELUM status: bila `wide` belum selesai pada pembacaan kedua, ia juga belum selesai pada
      // pembacaan pertama, jadi `narrow` memang tidak boleh punya attempt.
      const started = attempts(run).filter((row) => row.task === narrow);
      const rows = taskRows(run);
      if (!TERMINAL.has(rows[wide])) expect(started).toHaveLength(0);
      return TERMINAL.has(rows[wide]) && TERMINAL.has(rows[narrow]);
    }, 'overlap run to finish');

    expect(taskRows(run)).toEqual({ [wide]: 'DONE', [narrow]: 'DONE' });
    const rows = attempts(run);
    expect(rows).toHaveLength(2);
    expect(overlaps(rows[0], rows[1])).toBe(false);
    expect(rows[0].task).toBe(wide);
    expect(integrationSubjects(run)).toEqual([`integrate ${narrow}`, `integrate ${wide}`]);
    expectBaseIntact();
  });

  test('[matrix:review-retry] a rejected review retries once and integrates exactly once', async () => {
    const { run, id, statements } = createRun('retry', 100000, [
      { id: 'r', file: 'c.txt', to: 'c-done', reject: 1, maxAttempts: 2 }
    ]);
    sql(statements);
    const task = id({ id: 'r' });
    await settle(run, [task]);

    expect(taskRows(run)[task]).toBe('DONE');
    const rows = attempts(run);
    expect(rows.map((row) => [row.status, row.error])).toEqual([
      ['failed', 'review.changes_requested'],
      ['completed', '']
    ]);
    expect(integrationSubjects(run)).toEqual([`integrate ${task}`]);
    expectBaseIntact();
  });

  test('[matrix:semantic-conflict] a semantic regression on the integration branch needs a human and spares the base', async () => {
    // `winner` mengubah core.txt dan lolos lebih dulu; `loser` bersih di worktree-nya sendiri (core.txt masih v1)
    // tetapi pemeriksaan di cabang integrasi (core.txt sudah v2) gagal.
    const { run, id, statements } = createRun('regress', 100000, [
      { id: 'winner', file: 'core.txt', from: 'v1', to: 'v2', verify: 'grep -q v2 core.txt' },
      { id: 'loser', file: 'ui.txt', to: 'ui-done', verify: 'grep -q v1 core.txt', delay: 15000 }
    ]);
    sql(statements);
    const winner = id({ id: 'winner' });
    const loser = id({ id: 'loser' });
    await settle(run, [winner, loser]);

    expect(taskRows(run)).toEqual({ [winner]: 'DONE', [loser]: 'NEEDS_HUMAN' });
    expect(integrationSubjects(run)).toEqual([`integrate ${winner}`]);
    expect(git('show', `${integrationBranch(run)}:core.txt`)).toBe('v2');
    expect(git('show', `${integrationBranch(run)}:ui.txt`)).toBe('base');
    expect(sql(`SELECT count(*) FROM artifacts WHERE task_id='${loser}' AND kind='conflict_report'`)).toBe('1');
    expect(attempts(run).find((row) => row.task === loser).status).toBe('failed');
    expectBaseIntact();
  });

  test('[matrix:budget-stop] budget stop keeps dependent tasks from starting', async ({ request }) => {
    // Budget 10.000 (cadangan 15% => Stop pada 8.500). Task lama yang sudah selesai menghabiskan 5.600 token;
    // `first` memakai ~3.000 lagi (3 balasan x 1.000) sehingga run masuk level Stop. Reservasi masih cukup untuk
    // `second` dan `third`, jadi hanya level Stop yang menahan mereka.
    const spentAttempt = randomUUID();
    const { run, id, statements } = createRun(
      'budget',
      10000,
      [
        { id: 'first', file: 'a.txt', to: 'a-done', tokens: 1000 },
        { id: 'second', file: 'b.txt', to: 'b-done', deps: ['first'] },
        { id: 'third', file: 'c.txt', to: 'c-done', deps: ['first'] }
      ],
      (runId) => [
        `INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts) VALUES ('budget-spent-${suffix}','${runId}','worker','spent','spent','DONE','["d.txt"]','["works"]','["true"]',2000,1000,1)`,
        `INSERT INTO agent_runs (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,finished_at,retain_until) VALUES ('${spentAttempt}','budget-spent-${suffix}','worker','${providerId}','e2e-model',1,'completed','noctis-spent-${suffix}','${baseCommit}',now(),now(),now()+interval '1 hour')`,
        `INSERT INTO model_usage (agent_run_id,input_tokens,output_tokens,latency_ms) VALUES ('${spentAttempt}',5000,600,10)`
      ]
    );
    sql(statements);
    await settle(run, [id({ id: 'first' })]);
    expect(taskRows(run)[id({ id: 'first' })]).toBe('DONE');

    // Beri scheduler beberapa putaran (idle maksimum 2 dtk) untuk membuktikan task lain memang tidak diambil.
    await new Promise((resolve) => setTimeout(resolve, 7000));
    const rows = taskRows(run);
    expect(rows[id({ id: 'second' })]).toBe('READY');
    expect(rows[id({ id: 'third' })]).toBe('READY');
    expect(attempts(run).map((row) => row.task).filter((task) => !task.startsWith('budget-spent'))).toEqual([id({ id: 'first' })]);
    const view = await (await request.get(`/api/v1/runs/${run}/scheduler`)).json();
    expect(view.budget.used + view.budget.held).toBeGreaterThanOrEqual(view.budget.limit - view.budget.reserve);
    expectBaseIntact();
  });
});
