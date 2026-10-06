// Cek helper dashboard paralel: `node web/src/lib/components/workers/helpers.test.mjs` (Node 22.18+ membaca .ts).
import assert from 'node:assert/strict';
import { controlNotice, gaugePercent, heartbeatState, isValidGauge, padSlots, uniqueBy } from './helpers.ts';

assert.deepEqual([undefined, -1, NaN, 0, 29, 30, 59, 60, 600].map((age) => heartbeatState(age)), ['none', 'none', 'none', 'fresh', 'fresh', 'slow', 'slow', 'stale', 'stale']);
assert.equal(heartbeatState(10, 10), 'stale');

const gauge = (limit, used, held, reserve = 0) => ({ label: 'g', limit, used, held, estimated: false, reserve });
// Run: batas kerja = 1000 - 150 = 850; 595 = tepat 70%.
assert.equal(gaugePercent(gauge(1000, 500, 95, 150)), 70);
assert.equal(gaugePercent(gauge(1000, 850, 0, 150)), 100);
assert.ok(gaugePercent(gauge(1000, 900, 0, 150)) > 100, 'tidak dipotong di 100');
assert.equal(gaugePercent(gauge(100, 25, 25)), 50);
assert.equal(gaugePercent(gauge(100, 0, 0, 100)), 100, 'batas kerja nol dianggap penuh');

assert.equal(isValidGauge(gauge(1000, 1, 2, 150)), true);
for (const bad of [gauge(0, 0, 0), gauge(100, -1, 0), gauge(100, 1.5, 0), gauge(100, 0, 0, 100), gauge(Number.MAX_SAFE_INTEGER + 2, 0, 0)]) assert.equal(isValidGauge(bad), false);
// Slot: dilengkapi sampai kapasitas, tanpa duplikat, urut; run yang di-pause menandai slot kosong 'paused'.
const active = (slot) => ({ slot, state: 'running', task_id: `t${slot}`, leases: [] });
assert.deepEqual(padSlots([active(2)], 4, 'RUNNING').map((s) => [s.slot, s.state]), [[1, 'idle'], [2, 'running'], [3, 'idle'], [4, 'idle']]);
assert.deepEqual(padSlots([], 2, 'PAUSED').map((s) => s.state), ['paused', 'paused']);
assert.deepEqual(padSlots([active(1), active(1), active(2)], 2, 'RUNNING').map((s) => s.slot), [1, 2], 'nomor slot ganda dibuang');
assert.equal(padSlots([active(1), active(2), active(3)], 2, 'RUNNING').length, 3, 'aktif melebihi kapasitas tetap tampil');
assert.deepEqual(padSlots([], 0, 'RUNNING'), []);
const input = [active(2), active(1)];
assert.deepEqual(padSlots(input, 3, 'RUNNING'), padSlots(input, 3, 'RUNNING'), 'idempoten: refresh berulang menghasilkan keluaran sama');
assert.deepEqual(input.map((s) => s.slot), [2, 1], 'masukan tidak diubah');
assert.deepEqual(uniqueBy([{ k: 'a', v: 1 }, { k: 'a', v: 2 }, { k: 'b', v: 3 }], (x) => x.k).map((x) => x.v), [1, 3]);

// Pesan kontrol: hasil pause/cancel tetap terlihat selama masih ada pekerjaan.
assert.equal(controlNotice('RUNNING', 3), '');
assert.equal(controlNotice('PAUSED', 2), 'Run paused. 2 running tasks are finishing; nothing new will start.');
assert.equal(controlNotice('PAUSED', 1), 'Run paused. 1 running task is finishing; nothing new will start.');
assert.equal(controlNotice('PAUSED', 0), 'Run paused. No tasks are running.');
assert.equal(controlNotice('CANCELLED', 1), 'Run cancelled. Stopping 1 worker…');
assert.equal(controlNotice('CANCELLED', 0), 'Run cancelled. All workers have stopped.');
console.log('helpers.test.mjs OK');
