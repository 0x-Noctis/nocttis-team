// Cek helper approval: `node web/src/lib/components/approval/helpers.test.mjs` (Node 22.18+ membaca .ts).
import assert from 'node:assert/strict';
import { describeEvent, explainError, isStale, planRisks, validateDecision } from './helpers.ts';

assert.match(explainError('recovery.tool_in_progress'), /result is unknown/);
assert.match(explainError('recovery.side_effect_ambiguous'), /partly applied/);
assert.match(explainError('recovery.worktree_unverified'), /cannot be trusted/);
assert.equal(explainError('custom.code'), 'Last attempt ended with custom.code.');
assert.equal(explainError(null), 'No error was recorded for the last attempt.');

assert.equal(isStale({ error: { code: 'CONFLICT' } }), true);
assert.equal(isStale({ error: { code: 'NOT_FOUND' } }), false);
assert.equal(isStale(null), false);

assert.equal(validateDecision('', 'x', false), 'Enter who you are acting as before deciding.');
assert.equal(validateDecision('   ', 'x', false), 'Enter who you are acting as before deciding.');
assert.equal(validateDecision('a'.repeat(129), '', false), 'Actor name is too long (128 characters max).');
assert.equal(validateDecision('alice', '  ', true), 'A reason is required.');
assert.equal(validateDecision('alice', '  ', false), null);
assert.equal(validateDecision('alice', 'x'.repeat(501), false), 'The reason is too long (500 characters max).');
assert.equal(validateDecision('alice', 'ok', true), null);

const plan = (over, flags = []) => ({ risk_flags: flags, over_budget: over, reserved_tokens: 6000, available_tokens: 1000 });
assert.deepEqual(planRisks(plan(false)), []);
assert.deepEqual(planRisks(plan(false, ['shared module'])), ['shared module']);
const risky = planRisks(plan(true, ['shared module']));
assert.equal(risky.length, 2);
assert.match(risky[1], /Needs 6,000 tokens but only 1,000 remain/);

const event = (over) => ({ id: 1, event_type: 'status_transition', actor: 'human', actor_id: 'carol', from_status: 'NEEDS_HUMAN', to_status: 'READY', payload: {}, created_at: 'x', ...over });
assert.equal(describeEvent(event({ payload: { reason: 'fixed' } })), 'carol (human) · status_transition: NEEDS_HUMAN → READY — fixed');
assert.equal(describeEvent(event({ actor: 'system', actor_id: null, event_type: 'recovery', from_status: null, to_status: null, payload: { disposition: 'recovery_required' } })), 'system · recovery — recovery_required');
assert.equal(describeEvent(event({ actor_id: null })), 'human · status_transition: NEEDS_HUMAN → READY');
console.log('approval helpers.test.mjs OK');
