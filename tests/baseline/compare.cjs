// M5-010/M5-015: bandingkan baseline single-agent (results.json) dengan hasil MVP (mvp-results*.json) dan hitung gate rilis.
//
// Metrik DIPISAH menurut jenis skenario:
//   - skenario normal  : harus selesai otomatis (sukses, tanpa intervensi). Syarat: >= 80% dari semua run.
//   - skenario konflik : tujuannya BUKAN selesai otomatis, melainkan berhenti dengan aman: meminta keputusan manusia
//                        dan tidak merusak branch dasar. Syarat: 100% run. Tidak pernah dihitung sebagai sukses otomatis,
//                        dan tidak dicampur dengan kegagalan worker biasa.
//   - penghematan token: median input token run NORMAL MVP vs median skenario normal baseline. Syarat: turun >= 50%.
//   - branch dasar     : SEMUA run (normal dan konflik) wajib `base_intact === true`. Syarat: 100%.
//
// Mendukung beberapa putaran (`round`) per skenario. Dipisah dari kolektor (mvp-run.cjs) agar bisa dites tanpa model.
// Pakai: node tests/baseline/compare.cjs [baseline.json] [mvp-results.json ...]
const fs = require('node:fs');
const path = require('node:path');

const MIN_NORMAL_SUCCESS = 0.8;
const MIN_TOKEN_REDUCTION = 0.5;
const REQUIRED_CONFLICT_ESCALATION = 1;
const REQUIRED_BASE_INTACT = 1;
/** Skenario yang tujuannya berhenti aman, bukan selesai otomatis (acceptance-nya tidak dapat dipenuhi satu worker). */
const DEFAULT_CONFLICT_SCENARIOS = ['file-conflict'];

const sum = (runs, field) => runs.reduce((total, run) => total + run[field], 0);

/** Median angka; untuk jumlah genap dipakai rata-rata dua nilai tengah. */
function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

/** Selesai otomatis = sukses dan tanpa intervensi manusia. */
const automatic = (run) => run.success === true && run.human_interventions === 0;

/** Ringkasan satu set run (definisi metrik sama untuk baseline dan MVP). */
function summarize(runs) {
  if (runs.length === 0) throw new Error('tidak ada run untuk diringkas');
  const priced = runs.every((run) => typeof run.cost_micros === 'number');
  return {
    runs: runs.length,
    success_rate: runs.filter((run) => run.success).length / runs.length,
    automatic_rate: runs.filter(automatic).length / runs.length,
    median_input_tokens: median(runs.map((run) => run.input_tokens)),
    total_input_tokens: sum(runs, 'input_tokens'),
    total_output_tokens: sum(runs, 'output_tokens'),
    total_latency_seconds: sum(runs, 'latency_seconds'),
    retries: sum(runs, 'retries'),
    conflicts: sum(runs, 'conflicts'),
    tests_passed: sum(runs, 'tests_passed'),
    tests_failed: sum(runs, 'tests_failed'),
    human_interventions: sum(runs, 'human_interventions'),
    // Biaya hanya dilaporkan bila SEMUA run punya tarif; angka karangan lebih buruk daripada N/A.
    cost_micros: priced ? sum(runs, 'cost_micros') : null
  };
}

/** Hentikan aman = berhenti tanpa selesai, meminta keputusan manusia, dan branch dasar utuh. */
const stoppedSafely = (run) => run.success !== true && run.escalated_safely === true && run.base_intact === true;

/**
 * Bandingkan baseline dengan MVP (boleh banyak putaran). `conflictScenarios` menentukan skenario berjenis konflik.
 * Mengembalikan ringkasan per jenis, nilai tiap gate, dan hasil lulus/gagalnya.
 */
function compare(baselineRuns, mvpRuns, conflictScenarios = DEFAULT_CONFLICT_SCENARIOS) {
  const conflictIds = new Set(conflictScenarios);
  const ids = (runs) => [...new Set(runs.map((run) => run.scenario))].sort().join(',');
  if (ids(baselineRuns) !== ids(mvpRuns)) throw new Error('skenario baseline dan MVP tidak sama');
  const isConflict = (run) => conflictIds.has(run.scenario);
  const baseNormal = baselineRuns.filter((run) => !isConflict(run));
  const mvpNormal = mvpRuns.filter((run) => !isConflict(run));
  const mvpConflict = mvpRuns.filter(isConflict);
  if (baseNormal.length === 0 || mvpNormal.length === 0) throw new Error('tidak ada skenario normal');

  const normal = summarize(mvpNormal);
  const baseline = summarize(baseNormal);
  const reduction = 1 - normal.median_input_tokens / baseline.median_input_tokens;
  const escalated = mvpConflict.filter(stoppedSafely).length;
  const intact = mvpRuns.filter((run) => run.base_intact === true).length;
  const gate = (actual, required) => ({ required, actual, pass: actual >= required });
  return {
    baseline,
    normal,
    conflict: { runs: mvpConflict.length, stopped_safely: escalated, rate: mvpConflict.length ? escalated / mvpConflict.length : null },
    base_intact: { runs: mvpRuns.length, intact, rate: intact / mvpRuns.length },
    median_input_reduction: reduction,
    gates: {
      normal_success: gate(normal.automatic_rate, MIN_NORMAL_SUCCESS),
      token_reduction: gate(reduction, MIN_TOKEN_REDUCTION),
      // Tanpa run konflik tidak ada bukti: gagal tertutup.
      conflict_escalation: mvpConflict.length ? gate(escalated / mvpConflict.length, REQUIRED_CONFLICT_ESCALATION) : { required: REQUIRED_CONFLICT_ESCALATION, actual: 0, pass: false },
      base_branch_intact: gate(intact / mvpRuns.length, REQUIRED_BASE_INTACT)
    }
  };
}

const pct = (value) => `${(value * 100).toFixed(1)}%`;
const money = (micros) => (micros === null ? 'N/A' : `$${(micros / 1e6).toFixed(4)}`);

/** Tabel Markdown per skenario: baseline vs MVP (median token antar putaran, k/n selesai otomatis). */
function table(baselineRuns, mvpRuns, conflictScenarios = DEFAULT_CONFLICT_SCENARIOS) {
  const conflictIds = new Set(conflictScenarios);
  const rows = baselineRuns.map((base) => {
    const runs = mvpRuns.filter((run) => run.scenario === base.scenario);
    const kind = conflictIds.has(base.scenario) ? 'konflik' : 'normal';
    const good = conflictIds.has(base.scenario) ? runs.filter(stoppedSafely).length : runs.filter(automatic).length;
    const label = conflictIds.has(base.scenario) ? 'berhenti aman' : 'otomatis';
    return `| ${base.scenario} | ${kind} | ${base.input_tokens} → ${median(runs.map((run) => run.input_tokens))} | ${base.output_tokens} → ${median(runs.map((run) => run.output_tokens))} | ${good}/${runs.length} ${label} |`;
  });
  return [
    '| Scenario | Jenis | Input token (baseline → median MVP) | Output token | Hasil MVP |',
    '|---|---|---:|---:|---|',
    ...rows
  ].join('\n');
}

function loadRuns(file) {
  const data = JSON.parse(fs.readFileSync(file, 'utf8'));
  return { data, runs: data.runs };
}

function main() {
  const [baselinePath = path.join(__dirname, 'results.json'), ...mvpPaths] = process.argv.slice(2);
  const files = mvpPaths.length ? mvpPaths : [path.join(__dirname, 'mvp-results.json')];
  const missing = files.find((file) => !fs.existsSync(file));
  if (missing) {
    console.error(`belum ada data MVP: ${missing} (jalankan tests/baseline/mvp-run.cjs dengan model nyata)`);
    process.exit(2);
  }
  const baseline = loadRuns(baselinePath);
  const mvp = files.map(loadRuns);
  const mvpRuns = mvp.flatMap(({ runs }, fileIndex) => runs.map((run) => ({ ...run, source: files[fileIndex] })));
  const result = compare(baseline.runs, mvpRuns);
  console.log(`baseline: ${baseline.data.model} (${baseline.data.date}) | mvp: ${mvp[0].data.model} (${mvp.map(({ data }) => data.date).join(', ')}), ${mvpRuns.length} run dari ${files.length} berkas\n`);
  console.log(table(baseline.runs, mvpRuns));
  console.log(`\nskenario normal: ${result.normal.runs} run, selesai otomatis ${pct(result.normal.automatic_rate)}`);
  console.log(`median input token normal: ${result.baseline.median_input_tokens} → ${result.normal.median_input_tokens} (turun ${pct(result.median_input_reduction)})`);
  console.log(`skenario konflik: ${result.conflict.stopped_safely}/${result.conflict.runs} berhenti aman dan meminta manusia`);
  console.log(`branch dasar utuh: ${result.base_intact.intact}/${result.base_intact.runs} run`);
  console.log(`biaya: ${money(result.baseline.cost_micros)} → ${money(result.normal.cost_micros)}`);
  for (const [name, gate] of Object.entries(result.gates)) {
    console.log(`${gate.pass ? 'PASS' : 'FAIL'} ${name}: ${pct(gate.actual)} (minimal ${pct(gate.required)})`);
  }
  process.exit(Object.values(result.gates).every((gate) => gate.pass) ? 0 : 1);
}

if (require.main === module) main();
module.exports = { median, summarize, compare, table, stoppedSafely, automatic };
