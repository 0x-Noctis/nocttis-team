// Test bagian kolektor mvp-run.cjs yang tidak butuh backend/model: penjumlahan usage dan verifikasi branch integrasi.
// Data usage di sini SINTETIS; repository uji dibuat dari template fixture sungguhan di direktori sementara.
const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { test } = require('node:test');
const { addUsage, verifyIntegration } = require('./mvp-run.cjs');

const template = path.join(__dirname, '..', 'fixtures', 'sample-project', 'template');
const git = (dir, ...args) => {
  const result = spawnSync('git', ['-C', dir, '-c', 'user.name=t', '-c', 'user.email=t@example.invalid', ...args], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
};

test('addUsage: JSON biasa dan SSE dihitung satu request, cached ikut, tanpa usage diabaikan', () => {
  const totals = { input: 0, cached: 0, output: 0, requests: 0 };
  addUsage(totals, JSON.stringify({ usage: { prompt_tokens: 100, completion_tokens: 7, prompt_tokens_details: { cached_tokens: 40 } } }));
  addUsage(totals, 'data: {"choices":[]}\n\ndata: {"usage":{"prompt_tokens":10,"completion_tokens":3}}\n\ndata: [DONE]\n');
  addUsage(totals, JSON.stringify({ error: 'x' }));
  assert.deepEqual(totals, { input: 110, cached: 40, output: 10, requests: 2 });
});

/** Repository dari template dengan branch integrasi; `mutate` mengubah file pada branch itu sebelum commit. */
function repoWithIntegration(runId, mutate) {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'noctis-bench-test-'));
  fs.cpSync(template, repo, { recursive: true });
  git(repo, 'init', '--quiet', '--initial-branch=main');
  git(repo, 'add', '.');
  git(repo, 'commit', '--quiet', '-m', 'base');
  git(repo, 'checkout', '--quiet', '-b', `noctis-integration-${runId}`);
  mutate(repo);
  git(repo, 'add', '.');
  git(repo, 'commit', '--quiet', '--allow-empty', '-m', 'integrate');
  git(repo, 'checkout', '--quiet', 'main');
  return repo;
}

test('verifyIntegration: test hijau, test merah, dan branch tidak ada', () => {
  const green = repoWithIntegration('green', () => {});
  const red = repoWithIntegration('red', (repo) => {
    fs.writeFileSync(path.join(repo, 'test', 'extra.test.js'), "import test from 'node:test'; import assert from 'node:assert/strict'; test('merah', () => assert.equal(1, 2));\n");
  });
  try {
    const ok = verifyIntegration(green, 'green', 'node --test');
    assert.equal(ok.ran, true);
    assert.equal(ok.failed, 0);
    assert.ok(ok.passed > 0);
    const bad = verifyIntegration(red, 'red', 'node --test');
    assert.ok(bad.failed > 0);
    assert.deepEqual(verifyIntegration(green, 'tidak-ada', 'node --test'), { passed: 0, failed: 0, ran: false });
  } finally {
    fs.rmSync(green, { recursive: true, force: true });
    fs.rmSync(red, { recursive: true, force: true });
  }
});
