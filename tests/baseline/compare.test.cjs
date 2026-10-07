// Test perhitungan compare.cjs. SEMUA angka di sini SINTETIS (hanya untuk menguji rumus), bukan hasil ukur.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { test } = require('node:test');
const { compare, median, summarize } = require('./compare.cjs');

const SCENARIOS = ['backend-only', 'frontend-only', 'cross-stack', 'test-failure', 'file-conflict'];
const run = (scenario, input_tokens, overrides = {}) => ({
  scenario, success: true, input_tokens, output_tokens: 100, latency_seconds: 10, retries: 0, conflicts: 0,
  tests_passed: 1, tests_failed: 0, human_interventions: 0, ...overrides
});
const runs = (tokens, overrides = []) => SCENARIOS.map((id, i) => run(id, tokens[i], overrides[i]));

test('median: ganjil, genap, dan tidak bergantung urutan input', () => {
  assert.equal(median([5, 1, 3]), 3);
  assert.equal(median([4, 1, 3, 2]), 2.5);
});

test('baseline nyata: median input token 61.118', () => {
  const baseline = JSON.parse(fs.readFileSync(path.join(__dirname, 'results.json'), 'utf8'));
  assert.equal(summarize(baseline.runs).median_input_tokens, 61118);
});

test('gate token: tepat 50% lulus, di bawahnya gagal', () => {
  const base = runs([100, 100, 100, 100, 100]);
  assert.equal(compare(base, runs([50, 50, 50, 50, 50])).gates.median_input_reduction.pass, true);
  assert.equal(compare(base, runs([51, 51, 51, 51, 51])).gates.median_input_reduction.pass, false);
  // Median, bukan rata-rata: dua outlier besar tidak boleh menutupi tiga skenario yang turun.
  assert.equal(compare(base, runs([10, 10, 10, 500, 500])).gates.median_input_reduction.pass, true);
});

test('gate intervensi: gagal atau butuh manusia = tidak dihitung unattended; 4 dari 5 lulus, 3 dari 5 gagal', () => {
  const base = runs([100, 100, 100, 100, 100]);
  const four = runs([40, 40, 40, 40, 40], [{ success: false }]);
  assert.equal(compare(base, four).gates.unattended_rate.pass, true);
  const three = runs([40, 40, 40, 40, 40], [{ success: false }, { human_interventions: 1 }]);
  const result = compare(base, three);
  assert.equal(result.gates.unattended_rate.actual, 0.6);
  assert.equal(result.gates.unattended_rate.pass, false);
});

test('biaya N/A kecuali semua run punya tarif', () => {
  assert.equal(summarize(runs([1, 1, 1, 1, 1])).cost_micros, null);
  assert.equal(summarize(runs([1, 1, 1, 1, 1], SCENARIOS.map(() => ({ cost_micros: 7 })))).cost_micros, 35);
  assert.equal(summarize(runs([1, 1, 1, 1, 1], [{ cost_micros: 7 }])).cost_micros, null);
});

test('skenario yang tidak sama ditolak', () => {
  assert.throws(() => compare(runs([1, 1, 1, 1, 1]), runs([1, 1, 1, 1, 1]).slice(1)), /tidak sama/);
});
