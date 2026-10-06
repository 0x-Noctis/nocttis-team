const { spawn, spawnSync } = require('node:child_process');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');

const root = process.cwd();
const runtime = fs.mkdtempSync(path.join(os.tmpdir(), 'noctis-e2e-'));
const repository = path.join(runtime, 'repository');
const worktrees = path.join(runtime, 'worktrees');
const artifacts = path.join(runtime, 'artifacts');
const postgres = 'noctis-agent-3-e2e-postgres';
const children = [];
let stopping = false;

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: root, encoding: 'utf8', ...options });
  if (result.status !== 0) throw new Error(`${command} failed`);
  return result.stdout.trim();
}

function start(command, args, options = {}) {
  const child = spawn(command, args, { cwd: root, stdio: 'inherit', ...options });
  children.push(child);
  child.on('exit', (code, signal) => {
    if (!stopping && code !== 0) {
      console.error(`${command} exited: ${code ?? signal}`);
      cleanup(1);
    }
  });
  return child;
}

async function wait(url, timeout = 120_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await new Promise((resolve) => {
      const request = http.get(url, (response) => {
        response.resume();
        resolve((response.statusCode ?? 500) < 500);
      });
      request.on('error', () => resolve(false));
      request.setTimeout(500, () => request.destroy());
    })) return;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`service unavailable: ${new URL(url).host}`);
}

async function waitPostgres(timeout = 30_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const result = spawnSync('docker', ['exec', postgres, 'pg_isready', '-U', 'ai_team', '-d', 'ai_team']);
    if (result.status === 0) return;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error('PostgreSQL fixture unavailable');
}

function cleanup(code) {
  if (stopping) return;
  stopping = true;
  for (const child of children.reverse()) child.kill('SIGTERM');
  spawnSync('docker', ['rm', '--force', '--volumes', postgres], { cwd: root, stdio: 'ignore' });
  fs.rmSync(runtime, { recursive: true, force: true });
  process.exit(code);
}

async function main() {
  spawnSync('docker', ['rm', '--force', '--volumes', postgres], { stdio: 'ignore' });
  run('docker', [
    'run', '--detach', '--name', postgres, '--publish', '127.0.0.1:55432:5432',
    '--env', 'POSTGRES_DB=ai_team', '--env', 'POSTGRES_USER=ai_team',
    '--env', 'POSTGRES_PASSWORD=ai_team_dev', 'postgres:16-alpine'
  ]);
  await waitPostgres();
  fs.mkdirSync(repository);
  run('git', ['init', '--initial-branch=main', repository]);
  run('git', ['-C', repository, 'config', 'user.name', 'Noctis E2E']);
  run('git', ['-C', repository, 'config', 'user.email', 'noctis@example.invalid']);
  fs.writeFileSync(path.join(repository, 'e2e-output.txt'), 'base\n');
  run('git', ['-C', repository, 'add', 'e2e-output.txt']);
  run('git', ['-C', repository, 'commit', '-m', 'initial fixture']);

  start('node', ['tests/e2e/fake-provider.cjs']);
  await wait('http://127.0.0.1:7411/health', 30_000);
  start('cargo', ['run', '--bin', 'ai-team'], {
    env: {
      ...process.env,
      DATABASE_URL: 'postgres://ai_team:ai_team_dev@127.0.0.1:55432/ai_team',
      PRIMARY_API_KEY: process.env.NOCTIS_E2E_API_KEY,
      NOCTIS_PROVIDER_HOST_ALLOWLIST: '127.0.0.1',
      NOCTIS__SERVER__BIND: '127.0.0.1:7410',
      NOCTIS__GIT__WORKTREE_ROOT: worktrees,
      NOCTIS__ARTIFACTS__ROOT: artifacts,
      NOCTIS__PROVIDER__BASE_URL: 'http://127.0.0.1:7411/v1',
      NOCTIS__PROVIDER__MODEL: 'e2e-model',
      // M4-010: suite paralel butuh 4 slot (skenario 2 dan 4 worker).
      NOCTIS__SCHEDULER__MAX_PARALLEL_AGENTS: '4',
      NOCTIS_PROVIDER_HOST_ALLOWLIST: '127.0.0.1',
      NOCTIS_RUNNER_IMAGE: 'rust:1'
    }
  });
  await wait('http://127.0.0.1:7410/api/v1/health');

  const sql = `
    INSERT INTO projects (id,name,repository_path)
    VALUES ('00000000-0000-4000-8000-000000000015','e2e','${repository.replaceAll("'", "''")}');
    INSERT INTO project_runs (id,project_id,objective,status,token_budget)
    VALUES ('00000000-0000-4000-8000-000000000016','00000000-0000-4000-8000-000000000015','e2e','running',100000);
  `;
  run('docker', ['exec', '-i', postgres, 'psql', '-U', 'ai_team', '-d', 'ai_team', '-v', 'ON_ERROR_STOP=1'], { input: sql });

  start('npm', ['run', 'dev', '--', '--port', '4174', '--strictPort'], { cwd: path.join(root, 'web') });
  await wait('http://127.0.0.1:4174', 30_000);
  start('node', ['tests/e2e/gateway.cjs']);
  await wait('http://127.0.0.1:4173', 30_000);
}

for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => cleanup(0));
process.on('uncaughtException', (error) => {
  console.error(error.message);
  cleanup(1);
});
void main();
