// Smoke browser terhadap stack Compose yang sudah berjalan (M5-007): UI statis dan API satu origin.
//   node web/scripts/compose-smoke.mjs [baseUrl]    # bawaan http://127.0.0.1:7410
// Memakai Playwright dari web/node_modules; gagal (exit 1) bila ada asersi yang tidak terpenuhi.
import { chromium } from '@playwright/test';
import assert from 'node:assert/strict';

const base = process.argv[2] ?? 'http://127.0.0.1:7410';
const browser = await chromium.launch();
const page = await browser.newPage();
const problems = [];
page.on('pageerror', (error) => problems.push(`pageerror: ${error.message}`));
page.on('console', (message) => message.type() === 'error' && problems.push(`console: ${message.text()}`));
page.on('requestfailed', (request) => problems.push(`requestfailed: ${request.url()}`));

try {
  // 1. Beranda dilayani statis dengan header keamanan.
  const home = await page.goto(base);
  assert.equal(home.status(), 200);
  const headers = home.headers();
  assert.equal(headers['x-content-type-options'], 'nosniff');
  assert.equal(headers['x-frame-options'], 'DENY');
  assert.match(headers['content-security-policy'], /frame-ancestors 'none'/);
  await page.getByRole('heading', { name: 'AI Team' }).waitFor();

  // 2. Muat langsung rute SPA (fallback index.html) dan data nyata dari API satu origin.
  await page.goto(`${base}/operations`);
  await page.getByRole('heading', { name: 'Operations', level: 1 }).waitFor();
  await page.getByRole('region', { name: 'Service health' }).getByRole('status').waitFor();
  const health = await page.getByRole('region', { name: 'Service health' }).getByRole('status').innerText();
  assert.match(health, /Ready/, `kesehatan: ${health}`);
  await page.goto(`${base}/settings`);
  await page.getByRole('heading', { name: 'Settings', level: 1 }).waitFor();

  // 3. Navigasi keyboard dari beranda ke Providers tetap bekerja di build produksi.
  await page.goto(base);
  await page.getByRole('heading', { name: 'AI Team' }).waitFor(); // tunggu halaman siap sebelum menekan tombol
  await page.keyboard.press('Tab');
  await page.keyboard.press('Enter');
  await page.waitForURL(/\/providers$/);
  await page.getByRole('heading', { name: 'Providers', exact: true }).waitFor();

  // 4. /api tidak pernah mengembalikan HTML.
  const missing = await page.request.get(`${base}/api/v1/does-not-exist`);
  assert.equal(missing.status(), 404);
  assert.equal((await missing.json()).error.code, 'NOT_FOUND');
  const live = await page.request.get(`${base}/api/v1/health/live`);
  assert.equal(live.status(), 200);
  const ready = await page.request.get(`${base}/api/v1/health/ready`);
  assert.equal(ready.status(), 200);

  assert.deepEqual(problems, [], `masalah browser:\n${problems.join('\n')}`);
  console.log('compose smoke ok');
} finally {
  await browser.close();
}
