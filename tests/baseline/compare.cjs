// M5-010: bandingkan hasil baseline single-agent (results.json) dengan hasil MVP (mvp-results.json)
// dan hitung dua kriteria angka dari RANCANGAN §23: penurunan median input token >= 50% dan
// >= 80% skenario selesai tanpa intervensi manual.
//
// Dipisah dari kolektor (mvp-run.cjs) supaya perhitungannya bisa dites tanpa model/backend.
// Pakai: node tests/baseline/compare.cjs [baseline.json] [mvp-results.json]
const fs = require('node:fs');
const path = require('node:path');

const MIN_TOKEN_REDUCTION = 0.5;
const MIN_UNATTENDED_RATE = 0.8;

const sum = (runs, field) => runs.reduce((total, run) => total + run[field], 0);

/** Median angka; untuk jumlah genap dipakai rata-rata dua nilai tengah. */
function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

/** Ringkasan satu set run (baseline atau MVP) dengan definisi metrik yang sama. */
function summarize(runs) {
  if (runs.length === 0) throw new Error('tidak ada run untuk diringkas');
  const unattended = runs.filter((run) => run.success && run.human_interventions === 0).length;
  const priced = runs.every((run) => typeof run.cost_micros === 'number');
  return {
    scenarios: runs.length,
    success_rate: runs.filter((run) => run.success).length / runs.length,
    unattended_rate: unattended / runs.length,
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

/** Bandingkan dua set run pada skenario yang sama; kembalikan ringkasan, selisih, dan hasil gate. */
function compare(baselineRuns, mvpRuns) {
  const ids = (runs) => runs.map((run) => run.scenario).sort().join(',');
  if (ids(baselineRuns) !== ids(mvpRuns)) throw new Error('skenario baseline dan MVP tidak sama');
  const baseline = summarize(baselineRuns);
  const mvp = summarize(mvpRuns);
  const reduction = 1 - mvp.median_input_tokens / baseline.median_input_tokens;
  return {
    baseline,
    mvp,
    median_input_reduction: reduction,
    gates: {
      median_input_reduction: { required: MIN_TOKEN_REDUCTION, actual: reduction, pass: reduction >= MIN_TOKEN_REDUCTION },
      unattended_rate: { required: MIN_UNATTENDED_RATE, actual: mvp.unattended_rate, pass: mvp.unattended_rate >= MIN_UNATTENDED_RATE }
    }
  };
}

const pct = (value) => `${(value * 100).toFixed(1)}%`;
const money = (micros) => (micros === null ? 'N/A' : `$${(micros / 1e6).toFixed(4)}`);

/** Tabel Markdown per skenario (baseline → MVP), siap ditempel ke docs/benchmark.md. */
function table(baselineRuns, mvpRuns) {
  const rows = baselineRuns.map((base) => {
    const mvp = mvpRuns.find((run) => run.scenario === base.scenario);
    const cell = (field) => `${base[field]} → ${mvp[field]}`;
    return `| ${base.scenario} | ${base.success ? 'yes' : 'no'} → ${mvp.success ? 'yes' : 'no'} | ${cell('input_tokens')} | ${cell('output_tokens')} | ${cell('latency_seconds')} | ${cell('retries')} | ${cell('conflicts')} | ${cell('tests_passed')} | ${cell('human_interventions')} |`;
  });
  return [
    '| Scenario | Success | Input tokens | Output tokens | Latency (s) | Retry | Conflict | Tests passed | Intervention |',
    '|---|---|---:|---:|---:|---:|---:|---:|---:|',
    ...rows
  ].join('\n');
}

function main() {
  const [baselinePath = path.join(__dirname, 'results.json'), mvpPath = path.join(__dirname, 'mvp-results.json')] = process.argv.slice(2);
  if (!fs.existsSync(mvpPath)) {
    console.error(`belum ada data MVP: ${mvpPath} (jalankan tests/baseline/mvp-run.cjs dengan model nyata)`);
    process.exit(2);
  }
  const baseline = JSON.parse(fs.readFileSync(baselinePath, 'utf8'));
  const mvp = JSON.parse(fs.readFileSync(mvpPath, 'utf8'));
  const result = compare(baseline.runs, mvp.runs);
  console.log(`baseline: ${baseline.model} (${baseline.date}) | mvp: ${mvp.model} (${mvp.date})\n`);
  console.log(table(baseline.runs, mvp.runs));
  console.log(`\nmedian input token: ${result.baseline.median_input_tokens} → ${result.mvp.median_input_tokens} (turun ${pct(result.median_input_reduction)})`);
  console.log(`success rate: ${pct(result.baseline.success_rate)} → ${pct(result.mvp.success_rate)}; tanpa intervensi: ${pct(result.mvp.unattended_rate)}`);
  console.log(`biaya: ${money(result.baseline.cost_micros)} → ${money(result.mvp.cost_micros)}`);
  for (const [name, gate] of Object.entries(result.gates)) {
    console.log(`${gate.pass ? 'PASS' : 'FAIL'} ${name}: ${pct(gate.actual)} (minimal ${pct(gate.required)})`);
  }
  process.exit(Object.values(result.gates).every((gate) => gate.pass) ? 0 : 1);
}

if (require.main === module) main();
module.exports = { median, summarize, compare, table };
