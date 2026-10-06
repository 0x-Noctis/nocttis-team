// Cek helper dashboard paralel: `node web/src/lib/components/workers/helpers.test.mjs` (Node 22.18+ membaca .ts).
import assert from 'node:assert/strict';
import { gaugePercent, heartbeatState, isValidGauge } from './helpers.ts';

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
console.log('helpers.test.mjs OK');
