import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const results = JSON.parse(await readFile(new URL('results.json', import.meta.url)));
const expected = new Set(['backend-only', 'frontend-only', 'cross-stack', 'test-failure', 'file-conflict']);

assert.equal(results.runs.length, 5);
assert.deepEqual(new Set(results.runs.map(({ scenario }) => scenario)), expected);
assert.ok(results.command);
assert.ok(results.model);
assert.ok(results.date);
for (const run of results.runs) {
  for (const field of ['success', 'input_tokens', 'output_tokens', 'latency_seconds', 'retries', 'conflicts', 'tests_passed', 'tests_failed', 'human_interventions']) {
    assert.notEqual(run[field], undefined, `${run.scenario} missing ${field}`);
  }

  const events = (await readFile(new URL(`${run.scenario}.jsonl`, import.meta.url), 'utf8'))
    .trim()
    .split('\n')
    .map(JSON.parse);
  const completed = events.findLast(({ type }) => type === 'turn.completed');
  assert.equal(completed.usage.input_tokens, run.input_tokens);
  assert.equal(completed.usage.output_tokens, run.output_tokens);
}
