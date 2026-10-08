// M5-010: jalankan lima skenario fixture (tests/fixtures/sample-project) lewat MVP Noctis
// dan tulis tests/baseline/mvp-results.json dengan skema yang sama seperti baseline (results.json).
//
// Prasyarat (lihat docs/benchmark.md): backend jalan dengan database KOSONG (scheduler melayani semua run),
// NOCTIS_PROVIDER_HOST_ALLOWLIST berisi 127.0.0.1, dan env var API key provider terset di proses backend.
//
// Konfigurasi lewat environment (nilai secret TIDAK pernah dibaca/ditulis skrip ini):
//   NOCTIS_BENCH_UPSTREAM        base URL provider OpenAI-compatible, mis. http://host:port/v1   (wajib)
//   NOCTIS_BENCH_MODEL           nama model di provider                                          (wajib)
//   NOCTIS_BENCH_API_KEY_ENV     NAMA env var di proses backend yang memegang API key           (wajib)
//   NOCTIS_BENCH_MODEL_ID        ID model internal; harus sama dengan NOCTIS__PROVIDER__MODEL backend (default bench-model)
//   NOCTIS_BENCH_API             URL backend            (default http://127.0.0.1:7410)
//   NOCTIS_BENCH_POSTGRES        nama container Postgres untuk psql (default noctis-agent-3-e2e-postgres)
//   NOCTIS_BENCH_SCENARIOS       daftar id skenario dipisah koma (default semua); hasil parsial TIDAK boleh dilaporkan sebagai benchmark penuh
//   NOCTIS_BENCH_TIMEOUT_SECONDS batas tunggu per skenario (default 900)
//   NOCTIS_BENCH_OBJECTIVE_PREFIX awalan objective (hanya untuk validasi harness dengan fake provider)
//
// Token dihitung dua cara: (1) proxy lokal yang meneruskan request ke upstream dan menjumlahkan `usage`
// dari respons — mencakup SEMUA panggilan model termasuk Lead; (2) tabel model_usage — hanya worker/reviewer,
// karena usage Lead belum dicatat (ponytail di src/api/lead.rs). Selisihnya dilaporkan sebagai lead_input_tokens.
const { spawnSync } = require('node:child_process');
const crypto = require('node:crypto');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const { normalize } = require('./sse-normalize.cjs');

const fixtureDir = path.join(__dirname, '..', 'fixtures', 'sample-project');
const env = (name, fallback) => process.env[name] ?? fallback;
const API = env('NOCTIS_BENCH_API', 'http://127.0.0.1:7410');
const POSTGRES = env('NOCTIS_BENCH_POSTGRES', 'noctis-agent-3-e2e-postgres');
const TIMEOUT_MS = Number(env('NOCTIS_BENCH_TIMEOUT_SECONDS', '900')) * 1000;
const PROXY_PORT = 7420;
// Scheduler memilih model dari konfigurasi backend (NOCTIS__PROVIDER__MODEL); ID ini HARUS sama dengan nilai itu.
const MODEL_ID = env('NOCTIS_BENCH_MODEL_ID', 'bench-model');

/**
 * Proxy penghitung: meneruskan request apa adanya (termasuk Authorization), menghitung usage dari respons ASLI,
 * lalu menormalkan respons provider yang tidak mematuhi kontrak OpenAI (lihat sse-normalize.cjs) sebelum diberikan ke backend.
 */
function startProxy(upstream) {
  const totals = { input: 0, cached: 0, output: 0, requests: 0 };
  const target = new URL(upstream);
  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => {
      const requestBody = Buffer.concat(chunks);
      let wantsStream = false;
      try { wantsStream = JSON.parse(requestBody.toString('utf8')).stream === true; } catch { /* bukan JSON: teruskan apa adanya */ }
      const headers = { ...req.headers, host: target.host };
      const upstreamReq = (target.protocol === 'https:' ? require('node:https') : http).request(
        { hostname: target.hostname, port: target.port, path: target.pathname.replace(/\/$/, '') + req.url.replace(/^\/v1/, ''), method: req.method, headers },
        (upstreamRes) => {
          const body = [];
          upstreamRes.on('data', (chunk) => body.push(chunk));
          upstreamRes.on('end', () => {
            const text = Buffer.concat(body).toString('utf8');
            addUsage(totals, text);
            // Diagnosis (opsional): NOCTIS_BENCH_DUMP=<berkas> menambahkan request/respons (dipotong). Header tidak pernah dicatat.
            if (process.env.NOCTIS_BENCH_DUMP) {
              fs.appendFileSync(process.env.NOCTIS_BENCH_DUMP, `${JSON.stringify({ status: upstreamRes.statusCode, request: requestBody.toString('utf8').slice(0, 12000), response: text.slice(0, 6000) })}\n`);
            }
            // Hanya respons sukses yang dinormalkan; error provider diteruskan apa adanya.
            const ok = upstreamRes.statusCode >= 200 && upstreamRes.statusCode < 300;
            const out = ok ? normalize(wantsStream, upstreamRes.headers['content-type'], text) : { contentType: upstreamRes.headers['content-type'], body: text };
            res.writeHead(upstreamRes.statusCode, { ...(out.contentType ? { 'content-type': out.contentType } : {}), 'content-length': Buffer.byteLength(out.body) });
            res.end(out.body);
          });
        }
      );
      upstreamReq.on('error', () => res.writeHead(502).end());
      upstreamReq.end(requestBody);
    });
  });
  return new Promise((resolve) => server.listen(PROXY_PORT, '127.0.0.1', () => resolve({ server, totals })));
}

/** Ambil `usage` dari respons JSON biasa atau dari baris `data:` SSE; satu respons dihitung satu request. */
function addUsage(totals, text) {
  const candidates = text.trimStart().startsWith('{') ? [text] : text.split('\n').filter((l) => l.startsWith('data: {')).map((l) => l.slice(6));
  let found = false;
  for (const raw of candidates) {
    let usage;
    try { usage = JSON.parse(raw).usage; } catch { continue; }
    if (!usage || found) continue;
    found = true;
    totals.input += usage.prompt_tokens ?? 0;
    totals.cached += usage.prompt_tokens_details?.cached_tokens ?? 0;
    totals.output += usage.completion_tokens ?? 0;
  }
  if (found) totals.requests += 1;
}

async function api(method, route, body) {
  const response = await fetch(`${API}${route}`, {
    method,
    headers: { 'content-type': 'application/json', 'Idempotency-Key': crypto.randomUUID() },
    body: body === undefined ? undefined : JSON.stringify(body)
  });
  const text = await response.text();
  if (!response.ok) throw new Error(`${method} ${route} -> ${response.status}: ${text.slice(0, 300)}`);
  return text ? JSON.parse(text) : {};
}

/** SQL lewat psql di container; mengembalikan baris dipisah `|`. Input hanya UUID/konstanta dari skrip ini. */
function sql(statement) {
  const result = spawnSync('docker', ['exec', POSTGRES, 'psql', '-U', 'ai_team', '-d', 'ai_team', '-At', '-c', statement], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error('psql gagal');
  return result.stdout.trim().split('\n').filter(Boolean).map((line) => line.split('|'));
}

/** Salin fixture ke repository sementara (reset.sh membuat commit deterministik di .fixture-repo). */
function freshRepository() {
  const reset = spawnSync('sh', [path.join(fixtureDir, 'reset.sh')], { encoding: 'utf8' });
  if (reset.status !== 0) throw new Error('reset.sh gagal');
  const target = fs.mkdtempSync(path.join(os.tmpdir(), 'noctis-bench-'));
  fs.cpSync(path.join(fixtureDir, '.fixture-repo'), target, { recursive: true });
  return target;
}

/** Jalankan perintah verify skenario pada hasil integrasi; hitung pass/fail dari ringkasan node --test. */
function verifyIntegration(repository, runId, command) {
  const branch = `noctis-integration-${runId}`;
  const has = spawnSync('git', ['-C', repository, 'rev-parse', '--verify', branch], { encoding: 'utf8' });
  if (has.status !== 0) return { passed: 0, failed: 0, ran: false };
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'noctis-bench-verify-'));
  try {
    const archive = spawnSync(`git -C '${repository}' archive '${branch}' | tar -x -C '${dir}'`, { shell: true });
    if (archive.status !== 0) return { passed: 0, failed: 0, ran: false };
    // NODE_TEST_CONTEXT diwarisi bila kolektor dipanggil dari test runner dan mengubah format output node --test.
    const { NODE_TEST_CONTEXT: _context, ...cleanEnv } = process.env;
    const result = spawnSync('sh', ['-c', command], { cwd: dir, encoding: 'utf8', env: cleanEnv });
    const count = (label) => Number(new RegExp(`(?:ℹ|#) ${label} (\\d+)`).exec(`${result.stdout}${result.stderr}`)?.[1] ?? 0);
    return { passed: count('pass'), failed: count('fail'), ran: true };
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

async function registerModel(providerId, upstreamBase) {
  await api('POST', '/api/v1/providers', { id: providerId, base_url: upstreamBase, api_key_env: process.env.NOCTIS_BENCH_API_KEY_ENV, request_timeout_seconds: 120 });
  await api('POST', `/api/v1/providers/${providerId}/models`, {
    id: MODEL_ID, provider_id: providerId, remote_name: process.env.NOCTIS_BENCH_MODEL, class: 'coding',
    context_window: 64000, max_output_tokens: 8000,
    claimed_capabilities: { chat: true, streaming: true, tools: true, parallel_tools: false }
  });
  const probe = await api('POST', `/api/v1/models/${MODEL_ID}/probes/tools`);
  if (probe.result?.verified !== 'supported') throw new Error('model tidak mendukung tool calling');
}

async function runScenario(scenario, proxy) {
  const repository = freshRepository();
  const providerId = `bench-${crypto.randomUUID().slice(0, 8)}`;
  const projectId = crypto.randomUUID();
  const runId = crypto.randomUUID();
  let before = { ...proxy.totals };
  try {
    await registerModel(providerId, `http://127.0.0.1:${PROXY_PORT}/v1`);
    // Probe kemampuan tool adalah penyiapan, bukan kerja task: token-nya tidak boleh dihitung sebagai biaya MVP.
    before = { ...proxy.totals };
    await api('POST', '/api/v1/projects', { id: projectId, name: `bench ${scenario.id}`, repository_path: repository });
    const objective = `${env('NOCTIS_BENCH_OBJECTIVE_PREFIX', '')}${scenario.title}. Only these paths may change: ${scenario.allowed_paths.join(', ')}.`;
    await api('POST', `/api/v1/projects/${projectId}/runs`, { id: runId, project_id: projectId, objective, acceptance_criteria: scenario.acceptance, token_budget: 400000 });

    const started = Date.now();
    const { plan } = await api('POST', `/api/v1/runs/${runId}/lead-plan`, { model_id: MODEL_ID });
    // Persetujuan plan adalah gerbang kebijakan (RANCANGAN keputusan 5), bukan intervensi; dicatat terpisah.
    await api('POST', `/api/v1/runs/${runId}/approve-plan`, { plan_id: plan.id, actor_id: 'bench', decision: 'APPROVED', reason: null });
    let status = 'RUNNING';
    while (Date.now() - started < TIMEOUT_MS) {
      status = (await api('GET', `/api/v1/runs/${runId}`)).run.status;
      if (['DONE', 'FAILED', 'CANCELLED'].includes(status)) break;
      await new Promise((resolve) => setTimeout(resolve, 2000));
    }
    const latency = Math.round((Date.now() - started) / 1000);

    const used = {
      input_tokens: proxy.totals.input - before.input,
      cached_input_tokens: proxy.totals.cached - before.cached,
      output_tokens: proxy.totals.output - before.output
    };
    const [[dbInput = '0'] = []] = sql(`SELECT coalesce(sum(u.input_tokens),0) FROM model_usage u JOIN agent_runs a ON a.id=u.agent_run_id JOIN tasks t ON t.id=a.task_id WHERE t.project_run_id='${runId}'`);
    const [[retries = '0'] = []] = sql(`SELECT count(*) FROM agent_runs a JOIN tasks t ON t.id=a.task_id WHERE t.project_run_id='${runId}' AND a.attempt>1`);
    const [[conflicts = '0'] = []] = sql(`SELECT count(*) FROM tasks WHERE project_run_id='${runId}' AND status='CONFLICT'`);
    const tests = verifyIntegration(repository, runId, scenario.verify);
    return {
      scenario: scenario.id,
      success: status === 'DONE' && tests.ran && tests.failed === 0 && tests.passed > 0,
      run_status: status,
      ...used,
      db_input_tokens: Number(dbInput),
      lead_input_tokens: used.input_tokens - Number(dbInput),
      latency_seconds: latency,
      retries: Number(retries),
      conflicts: Number(conflicts),
      tests_passed: tests.passed,
      tests_failed: tests.failed,
      plan_approvals: 1,
      human_interventions: 0,
      changed_paths: spawnSync('git', ['-C', repository, 'diff', '--name-only', `main...noctis-integration-${runId}`], { encoding: 'utf8' }).stdout.split('\n').filter(Boolean),
      run_id: runId
    };
  } catch (error) {
    // Kegagalan satu skenario (mis. plan Lead ditolak validasi) adalah HASIL, bukan alasan membuang skenario lain.
    // Tanpa intervensi: kolektor tidak mengulang permintaan Lead atau memperbaiki apa pun secara manual.
    return {
      scenario: scenario.id,
      success: false,
      run_status: 'ERROR',
      error: String(error.message).slice(0, 400),
      input_tokens: proxy.totals.input - before.input,
      cached_input_tokens: proxy.totals.cached - before.cached,
      output_tokens: proxy.totals.output - before.output,
      db_input_tokens: 0,
      lead_input_tokens: proxy.totals.input - before.input,
      latency_seconds: 0,
      retries: 0,
      conflicts: 0,
      tests_passed: 0,
      tests_failed: 0,
      plan_approvals: 0,
      human_interventions: 0,
      changed_paths: [],
      run_id: runId
    };
  } finally {
    await api('DELETE', `/api/v1/providers/${providerId}`).catch(() => {});
    fs.rmSync(repository, { recursive: true, force: true });
  }
}

async function main() {
  for (const name of ['NOCTIS_BENCH_UPSTREAM', 'NOCTIS_BENCH_MODEL', 'NOCTIS_BENCH_API_KEY_ENV']) {
    if (!process.env[name]) throw new Error(`${name} wajib diisi (lihat docs/benchmark.md)`);
  }
  const only = process.env.NOCTIS_BENCH_SCENARIOS?.split(',').filter(Boolean);
  const scenarios = JSON.parse(fs.readFileSync(path.join(fixtureDir, 'scenarios', 'tasks.json'), 'utf8')).filter((scenario) => !only || only.includes(scenario.id));
  if (!scenarios.length) throw new Error('NOCTIS_BENCH_SCENARIOS tidak cocok dengan skenario mana pun');
  const proxy = await startProxy(process.env.NOCTIS_BENCH_UPSTREAM);
  const runs = [];
  try {
    for (const scenario of scenarios) {
      console.error(`skenario ${scenario.id}…`);
      runs.push(await runScenario(scenario, proxy));
    }
  } finally {
    proxy.server.close();
  }
  const out = path.join(__dirname, 'mvp-results.json');
  fs.writeFileSync(out, `${JSON.stringify({
    task_id: 'M5-010',
    date: new Date().toISOString().slice(0, 10),
    command: 'node tests/baseline/mvp-run.cjs',
    model: process.env.NOCTIS_BENCH_MODEL,
    fixture_commit: spawnSync('git', ['-C', path.join(fixtureDir, '.fixture-repo'), 'rev-parse', 'HEAD'], { encoding: 'utf8' }).stdout.trim(),
    pricing: null,
    runs
  }, null, 2)}\n`);
  console.error(`tulis ${out}`);
}

if (require.main === module) main().catch((error) => { console.error(error.message); process.exit(1); });
module.exports = { addUsage, verifyIntegration };
