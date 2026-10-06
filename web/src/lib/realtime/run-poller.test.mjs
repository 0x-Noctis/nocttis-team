// Cek poller: `node web/src/lib/realtime/run-poller.test.mjs` (Node 22.18+ membaca .ts langsung).
import assert from 'node:assert/strict';
import { startPoller } from './run-poller.ts';

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// 1. Tick lambat tidak pernah tumpang tindih, meski interval lebih pendek dari durasi tick.
{
  let active = 0, peak = 0, calls = 0;
  const poller = startPoller(async () => { calls++; active++; peak = Math.max(peak, active); await sleep(25); active--; }, { intervalMs: 5 });
  await sleep(150);
  poller.stop();
  assert.equal(peak, 1);
  assert.ok(calls >= 3, `calls=${calls}`);
}

// 2. Gagal -> onError dengan hitungan beruntun, polling tetap lanjut, lalu onRecover sekali saat pulih.
{
  const errors = [];
  let recovered = 0, calls = 0;
  const poller = startPoller(async () => { if (++calls <= 2) throw new Error('down'); }, {
    intervalMs: 2, maxBackoffMs: 10, onError: (_, n) => errors.push(n), onRecover: () => recovered++
  });
  await sleep(150);
  poller.stop();
  assert.deepEqual(errors, [1, 2]);
  assert.equal(recovered, 1);
  assert.ok(calls > 3);
}

// 3. 'stop' menghentikan polling; stop() menghentikan sebelum tick berikutnya.
{
  let calls = 0;
  startPoller(async () => (++calls === 2 ? 'stop' : undefined), { intervalMs: 3 });
  await sleep(80);
  assert.equal(calls, 2);
  let after = 0;
  const poller = startPoller(async () => { after++; }, { intervalMs: 3 });
  poller.stop();
  await sleep(30);
  assert.equal(after, 0);
}

// 4. Tab tersembunyi: tick dilewati; refresh() saat terlihat langsung menjalankan tick tanpa menunggu interval.
{
  let visible = false, calls = 0;
  const poller = startPoller(async () => { calls++; }, { intervalMs: 5, isVisible: () => visible });
  await sleep(40);
  assert.equal(calls, 0);
  visible = true;
  poller.refresh();
  await sleep(1);
  assert.ok(calls >= 1);
  poller.stop();
}
console.log('run-poller.test.mjs OK');
