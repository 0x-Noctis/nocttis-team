// Cek cepat logika graf: `node web/src/lib/components/board/graph.test.mjs` (Node 22.18+ membaca .ts langsung).
import assert from 'node:assert/strict';
import { blockedBy, columnOf, dependencyLevels, indexTasks } from './graph.ts';
import { budgetLevel, parseCriteria, worstCaseTokens } from '../project/types.ts';

const task = (id, status, depends_on = []) => ({ contract: { id, depends_on }, status });
const ids = (list) => list.map((t) => t.contract.id);

// Diamond: a -> (b, c) -> d, urutan input diacak.
const diamond = [task('d', 'PLANNED', ['b', 'c']), task('b', 'DONE', ['a']), task('a', 'DONE'), task('c', 'RUNNING', ['a'])];
const result = dependencyLevels(diamond);
assert.deepEqual(result.levels.map(ids), [['a'], ['b', 'c'], ['d']]);
assert.equal(result.unresolved.length, 0);

// Siklus dan dependency hilang tidak boleh masuk level mana pun.
const broken = dependencyLevels([task('x', 'PLANNED', ['y']), task('y', 'PLANNED', ['x']), task('z', 'PLANNED', ['ghost']), task('ok', 'READY')]);
assert.deepEqual(broken.levels.map(ids), [['ok']]);
assert.deepEqual(ids(broken.unresolved), ['x', 'y', 'z']);

// Kolom board: dependency belum DONE => blocked; DONE/CANCELLED tidak blocked; status aktif tidak bergantung dependency.
const byId = indexTasks(diamond);
assert.deepEqual(blockedBy(diamond[0], byId), [{ id: 'c', status: 'RUNNING' }]);
assert.deepEqual(blockedBy(task('q', 'READY', ['nope']), byId), [{ id: 'nope', status: 'MISSING' }]);
assert.equal(columnOf(diamond[0], byId), 'blocked');
assert.equal(columnOf(diamond[2], byId), 'closed');
assert.equal(columnOf(diamond[3], byId), 'in_progress');
assert.equal(columnOf(task('r', 'READY'), byId), 'queued');
assert.equal(columnOf(task('f', 'FAILED'), byId), 'attention');
assert.equal(columnOf(task('v', 'VERIFY'), byId), 'checking');

// Budget dan helper form.
assert.deepEqual([0, 69.9, 70, 85, 100, 140].map(budgetLevel), ['ok', 'ok', 'warning', 'checkpoint', 'exhausted', 'exhausted']);
const limits = (i, o, a) => ({ limits: { max_input_tokens: i, max_output_tokens: o, max_attempts: a } });
assert.equal(worstCaseTokens([limits(100, 50, 2), limits(10, 5, 1)]), 315);
assert.equal(worstCaseTokens([limits(Number.MAX_SAFE_INTEGER, 1, 2)]), null);
assert.deepEqual(parseCriteria(' a \n\n  \nb\n'), ['a', 'b']);
console.log('graph.test.mjs OK');
