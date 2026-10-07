// M5-012: audit kriteria rilis "setiap task DONE memiliki review, verification, patch, event, dan usage".
// Berjalan TERAKHIR (nama zz-) pada database yang sudah diisi seluruh spec E2E lain, jadi mengaudit hasil orkestrasi nyata.
//
// Aturan: task DONE yang punya event (events) = diproses sistem, wajib punya SEMUA bukti. Sistem menulis event pada setiap
// perubahan status, sedangkan task hasil seeding fixture (INSERT ... 'DONE' di spec lain, termasuk `budget-spent` yang
// diberi usage buatan) tidak punya event sama sekali. Yang seperti itu dilaporkan jumlahnya, tidak diabaikan diam-diam.
const { expect, test } = require('../../web/node_modules/@playwright/test');
const { spawnSync } = require('node:child_process');

const POSTGRES = 'noctis-agent-3-e2e-postgres';
const COLUMNS = ['id', 'title', 'events', 'diff', 'transition', 'verification', 'worker_usage', 'reviewer_usage'];

/** Daftar bukti yang kurang pada satu baris audit (kosong = lengkap). */
function missingEvidence(row) {
  const missing = [];
  if (row.diff < 1) missing.push('patch/diff artifact');
  if (row.transition < 1) missing.push('status_transition event');
  if (row.verification < 1) missing.push('verification event');
  if (row.worker_usage < 1) missing.push('worker usage');
  if (row.reviewer_usage < 1) missing.push('review (reviewer usage)');
  return missing;
}

/** Task yang diproses sistem (punya event) dan bukti-nya tidak lengkap, beserta alasan. */
function incomplete(rows) {
  return rows.filter((row) => row.events > 0).map((row) => ({ id: row.id, title: row.title, missing: missingEvidence(row) })).filter((row) => row.missing.length);
}

function auditRows() {
  const statement = `SELECT t.id, t.title,
    (SELECT count(*) FROM events e WHERE e.task_id=t.id),
    (SELECT count(*) FROM artifacts a WHERE a.task_id=t.id AND a.kind='diff'),
    (SELECT count(*) FROM events e WHERE e.task_id=t.id AND e.event_type='status_transition'),
    (SELECT count(*) FROM events e WHERE e.task_id=t.id AND e.event_type='verification'),
    (SELECT count(*) FROM model_usage u JOIN agent_runs g ON g.id=u.agent_run_id WHERE g.task_id=t.id AND u.operation_key LIKE 'worker%'),
    (SELECT count(*) FROM model_usage u JOIN agent_runs g ON g.id=u.agent_run_id WHERE g.task_id=t.id AND u.operation_key LIKE 'reviewer%')
    FROM tasks t WHERE t.status='DONE' ORDER BY t.created_at`;
  const result = spawnSync('docker', ['exec', '-i', POSTGRES, 'psql', '-U', 'ai_team', '-d', 'ai_team', '-v', 'ON_ERROR_STOP=1', '-At', '-F', '|', '-c', statement], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(`psql failed: ${result.stderr}`);
  return result.stdout
    .trim()
    .split('\n')
    .filter(Boolean)
    .map((line) => Object.fromEntries(line.split('|').map((value, i) => [COLUMNS[i], i < 2 ? value : Number(value)])));
}

test('[release] audit detects every missing kind of evidence (synthetic rows)', () => {
  const complete = { id: 'a', title: 'ok', events: 4, diff: 1, transition: 3, verification: 1, worker_usage: 1, reviewer_usage: 1 };
  expect(incomplete([complete])).toEqual([]);
  for (const [field, label] of [['diff', 'patch/diff artifact'], ['transition', 'status_transition event'], ['verification', 'verification event'], ['worker_usage', 'worker usage'], ['reviewer_usage', 'review (reviewer usage)']]) {
    expect(incomplete([{ ...complete, [field]: 0 }])[0].missing).toEqual([label]);
  }
  // Tanpa event = fixture seeding; tidak dituntut bukti, tetapi tidak boleh menyembunyikan task yang punya attempt.
  expect(incomplete([{ ...complete, events: 0, diff: 0, verification: 0 }])).toEqual([]);
  expect(incomplete([complete, { ...complete, id: 'b', diff: 0 }]).map((row) => row.id)).toEqual(['b']);
});

test('[release] every DONE task processed by the system has review, verification, patch, event and usage', () => {
  const rows = auditRows();
  const processed = rows.filter((row) => row.events > 0);
  const seeded = rows.length - processed.length;
  console.log(`release-audit: DONE=${rows.length} processed=${processed.length} seeded-fixtures=${seeded}`);
  // Audit tanpa data bukan bukti: suite sebelumnya pasti menghasilkan task DONE lewat orkestrasi.
  expect(processed.length).toBeGreaterThan(0);
  expect(incomplete(rows)).toEqual([]);
});
