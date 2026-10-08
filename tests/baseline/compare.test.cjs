// Test perhitungan compare.cjs. SEMUA angka di sini SINTETIS (hanya untuk menguji rumus), bukan hasil ukur.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { test } = require('node:test');
const { compare, median, summarize } = require('./compare.cjs');

const SCENARIOS = ['backend-only', 'frontend-only', 'cross-stack', 'test-failure', 'file-conflict'];
const run = (scenario, input_tokens, overrides = {}) => ({
  scenario, success: true, input_tokens, output_tokens: 100, latency_seconds: 10, retries: 0, conflicts: 0,
  tests_passed: 1, tests_failed: 0, human_interventions: 0, base_intact: true, ...overrides
});
/** Satu putaran penuh: 4 skenario normal dengan token `tokens`, file-conflict berhenti aman (default). */
const round = (tokens, overrides = {}) =>
  SCENARIOS.map((id, i) =>
    id === 'file-conflict'
      ? run(id, 5000, { success: false, escalated_safely: true, ...overrides[id] })
      : run(id, tokens[i], overrides[id])
  );
const baseline = () => SCENARIOS.map((id) => run(id, 100));

test('median: ganjil, genap, dan tidak bergantung urutan input', () => {
  assert.equal(median([5, 1, 3]), 3);
  assert.equal(median([4, 1, 3, 2]), 2.5);
});

test('baseline nyata: median input token 61.118 (5 skenario)', () => {
  const data = JSON.parse(fs.readFileSync(path.join(__dirname, 'results.json'), 'utf8'));
  assert.equal(summarize(data.runs).median_input_tokens, 61118);
});

test('gate token dihitung dari skenario NORMAL: tepat 50% lulus, di bawahnya gagal; konflik tidak ikut menurunkan median', () => {
  assert.equal(compare(baseline(), round([50, 50, 50, 50])).gates.token_reduction.pass, true);
  assert.equal(compare(baseline(), round([51, 51, 51, 51])).gates.token_reduction.pass, false);
  // Run konflik bertoken kecil (5000) tidak boleh membuat median tampak lebih hemat.
  assert.equal(compare(baseline(), round([90, 90, 90, 90])).gates.token_reduction.pass, false);
});

test('gate normal: 4/4 lulus, 3/4 per putaran gagal, tetapi 10 dari 12 run lintas tiga putaran lulus', () => {
  const failOne = { 'test-failure': { success: false } };
  assert.equal(compare(baseline(), round([40, 40, 40, 40])).gates.normal_success.pass, true);
  assert.equal(compare(baseline(), round([40, 40, 40, 40], failOne)).gates.normal_success.actual, 0.75);
  // 3 putaran x 4 normal = 12 run; 2 gagal = 10/12 = 83% >= 80%.
  const rounds = [...round([40, 40, 40, 40]), ...round([40, 40, 40, 40], failOne), ...round([40, 40, 40, 40], failOne)];
  const result = compare(baseline(), rounds);
  assert.ok(Math.abs(result.gates.normal_success.actual - 10 / 12) < 1e-9);
  assert.equal(result.gates.normal_success.pass, true);
  // Intervensi manusia pada run yang sukses tidak dihitung otomatis.
  const manual = round([40, 40, 40, 40], { 'backend-only': { human_interventions: 1 } });
  assert.equal(compare(baseline(), manual).gates.normal_success.actual, 0.75);
});

test('konflik harus 100% berhenti aman; gagal biasa atau tanpa eskalasi tidak dihitung, dan tidak dicampur dengan metrik normal', () => {
  const safe = compare(baseline(), round([40, 40, 40, 40]));
  assert.equal(safe.gates.conflict_escalation.pass, true);
  assert.equal(safe.normal.runs, 4);
  const notEscalated = compare(baseline(), round([40, 40, 40, 40], { 'file-conflict': { escalated_safely: false } }));
  assert.equal(notEscalated.gates.conflict_escalation.pass, false);
  // Konflik yang "selesai sukses" tidak boleh diklaim sebagai berhenti aman.
  const claimedSuccess = compare(baseline(), round([40, 40, 40, 40], { 'file-conflict': { success: true } }));
  assert.equal(claimedSuccess.gates.conflict_escalation.pass, false);
  // Kegagalan konflik tidak menurunkan metrik normal.
  assert.equal(notEscalated.gates.normal_success.pass, true);
  // Salah satu dari dua putaran tidak aman => gagal.
  const mixed = compare(baseline(), [...round([40, 40, 40, 40]), ...round([40, 40, 40, 40], { 'file-conflict': { escalated_safely: false } })]);
  assert.equal(mixed.gates.conflict_escalation.actual, 0.5);
});

test('branch dasar harus utuh di SEMUA run; data yang hilang dihitung belum terbukti', () => {
  assert.equal(compare(baseline(), round([40, 40, 40, 40])).gates.base_branch_intact.pass, true);
  assert.equal(compare(baseline(), round([40, 40, 40, 40], { 'cross-stack': { base_intact: false } })).gates.base_branch_intact.pass, false);
  assert.equal(compare(baseline(), round([40, 40, 40, 40], { 'file-conflict': { base_intact: false } })).gates.base_branch_intact.pass, false);
  assert.equal(compare(baseline(), round([40, 40, 40, 40], { 'cross-stack': { base_intact: undefined } })).gates.base_branch_intact.pass, false);
  // Konflik yang merusak branch dasar tidak "berhenti aman".
  assert.equal(compare(baseline(), round([40, 40, 40, 40], { 'file-conflict': { base_intact: false } })).gates.conflict_escalation.pass, false);
});

test('skenario konflik yang hilang dari data MVP ditolak', () => {
  const normalOnly = round([40, 40, 40, 40]).filter((item) => item.scenario !== 'file-conflict');
  const base = baseline();
  assert.throws(() => compare(base, normalOnly), /tidak sama/);
});

test('biaya N/A kecuali semua run punya tarif', () => {
  assert.equal(summarize(round([1, 1, 1, 1])).cost_micros, null);
  assert.equal(summarize(SCENARIOS.map((id) => run(id, 1, { cost_micros: 7 }))).cost_micros, 35);
  assert.equal(summarize([run('a', 1, { cost_micros: 7 }), run('b', 1)]).cost_micros, null);
});

test('skenario yang tidak sama ditolak', () => {
  assert.throws(() => compare(baseline(), round([1, 1, 1, 1]).slice(1)), /tidak sama/);
});
