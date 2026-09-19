import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const fixtureRoot = new URL('./', import.meta.url);
const tasks = JSON.parse(await readFile(new URL('scenarios/tasks.json', fixtureRoot)));
const expected = new Set([
  'backend-only',
  'frontend-only',
  'cross-stack',
  'test-failure',
  'file-conflict',
]);

assert.deepEqual(new Set(tasks.map(({ id }) => id)), expected);
for (const task of tasks) {
  assert.ok(task.title);
  assert.ok(task.acceptance.length);
  assert.ok(task.allowed_paths.length);
  assert.ok(task.verify.startsWith('node --test'));
}
