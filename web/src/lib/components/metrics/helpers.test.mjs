// Cek helper halaman Operations: `node web/src/lib/components/metrics/helpers.test.mjs` (Node 22.18+ membaca .ts).
import assert from 'node:assert/strict';
import { describeComponent, estimatedPercent, formatConfigValue, formatCost, overallStatus, providerIssues, totalTokens } from './helpers.ts';

// Status selalu punya kata + ikon (bukan hanya warna) dan penjelasan khusus komponen.
for (const state of ['ok', 'degraded', 'down']) {
  for (const name of ['database', 'migrations', 'workers', 'providers']) {
    const view = describeComponent(name, state, { stale_attempts: 2 });
    assert.ok(view.label.length > 1 && view.icon.length === 1 && view.explanation.length > 10, `${name}/${state}`);
  }
}
assert.match(describeComponent('workers', 'degraded', { stale_attempts: 3 }).explanation, /^3 active attempt/);
assert.match(describeComponent('migrations', 'down', { stale_attempts: 0 }).explanation, /Do not run workers/);

const components = (overrides = {}) => ({ database: 'ok', migrations: 'ok', workers: 'ok', providers: 'ok', ...overrides });
assert.equal(overallStatus({ ready: true, components: components() }).label, 'Ready');
assert.equal(overallStatus({ ready: true, components: components({ providers: 'degraded' }) }).label, 'Ready, with warnings');
assert.equal(overallStatus({ ready: false, components: components({ database: 'down' }) }).label, 'Not ready');

assert.equal(totalTokens({ input_tokens: 900, output_tokens: 600 }), 1500);
assert.equal(estimatedPercent({ input_tokens: 900, output_tokens: 600, estimated_tokens: 500 }), 33);
assert.equal(estimatedPercent({ input_tokens: 0, output_tokens: 0, estimated_tokens: 0 }), 0, 'total nol');
assert.equal(estimatedPercent({ input_tokens: 1, output_tokens: 1, estimated_tokens: 999 }), 100, 'dipotong 100');

assert.match(formatCost(undefined), /^Not tracked/);
assert.match(formatCost({ tracked: false, micros: 0, priced_rows: 0, total_rows: 5 }), /^Not tracked/);
assert.equal(formatCost({ tracked: true, micros: 2500, priced_rows: 2, total_rows: 2 }), '0.0025 cost units (currency not recorded)');
assert.match(formatCost({ tracked: true, micros: 2500, priced_rows: 1, total_rows: 2 }), /covers 1 of 2 requests/);

const provider = (secret, tools) => ({ id: 'p', host: 'h', secret_configured: secret, models: tools.map((t) => ({ id: t, class: 'coding', tools: t })) });
assert.deepEqual(providerIssues(provider(true, ['supported'])), []);
assert.equal(providerIssues(provider(false, ['supported'])).length, 1);
assert.match(providerIssues(provider(true, []))[0], /No models/);
assert.match(providerIssues(provider(true, ['unknown', 'unsupported']))[0], /verified tool support/);
assert.equal(providerIssues(provider(false, [])).length, 2);

assert.equal(formatConfigValue(true), 'Yes');
assert.equal(formatConfigValue(false), 'No');
assert.equal(formatConfigValue(null), 'Not set');
assert.equal(formatConfigValue([]), 'None');
assert.equal(formatConfigValue(['a', 'b']), 'a, b');
assert.equal(formatConfigValue(4), '4');
console.log('metrics helpers ok');
