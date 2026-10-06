// Skenario deterministik untuk Lead Agent. Fake provider memilih skenario dari penanda
// `[scenario:<nama>]` di objective run; tanpa penanda dipakai `valid`.
// Semua ID memakai nilai dari brief (project_id/run_id) karena Lead wajib memakainya.

const LIMITS_SMALL = { max_input_tokens: 2000, max_output_tokens: 1000, max_tool_calls: 10, max_attempts: 1, timeout_seconds: 600 };
// 2 task x (60.000 + 10.000) = 140.000 > budget run E2E (100.000) sehingga Lead menolak plan.
const LIMITS_HUGE = { max_input_tokens: 60000, max_output_tokens: 10000, max_tool_calls: 10, max_attempts: 1, timeout_seconds: 600 };

function task(brief, id, title, path, dependsOn, limits = LIMITS_SMALL) {
  return {
    id,
    project_id: brief.project_id,
    project_run_id: brief.run_id,
    title,
    role: 'worker',
    objective: `${title} within its allowed path only.`,
    depends_on: dependsOn,
    allowed_paths: [path],
    context_refs: [],
    acceptance_criteria: [`${title} is complete`],
    // Harus persis salah satu test command hasil discovery, kalau tidak Lead menolak plan.
    verification_commands: [brief.discovery.test_commands[0]],
    limits
  };
}

const plan = (brief, tasks, riskFlags = []) => ({
  id: 'plan-1',
  project_run_id: brief.run_id,
  version: 1,
  tasks,
  risk_flags: riskFlags
});

const builders = {
  // Diamond: contract -> (backend, frontend) -> search-test. Tiga level dependency.
  valid: (brief) => plan(brief, [
    task(brief, 'contract', 'Define search contract', 'docs/contract.md', []),
    task(brief, 'backend', 'Implement search endpoint', 'src/backend.js', ['contract']),
    task(brief, 'frontend', 'Add search box', 'src/frontend.js', ['contract']),
    task(brief, 'search-test', 'Cover search with a test', 'test/search.test.js', ['backend', 'frontend'])
  ], ['backend and frontend share the products response shape']),

  // Siklus a <-> b: ProposedPlan::validate menolak graf dependency tidak valid.
  cycle: (brief) => plan(brief, [
    task(brief, 'a', 'Task A', 'src/backend.js', ['b']),
    task(brief, 'b', 'Task B', 'src/frontend.js', ['a'])
  ]),

  // Plan valid secara struktur tetapi melampaui token budget run.
  overbudget: (brief) => plan(brief, [
    task(brief, 'big-a', 'Large task A', 'src/backend.js', [], LIMITS_HUGE),
    task(brief, 'big-b', 'Large task B', 'src/frontend.js', [], LIMITS_HUGE)
  ])
};

const scenarioOf = (objective) => /\[scenario:([a-z-]+)\]/.exec(objective ?? '')?.[1] ?? 'valid';

// Request Lead dikenali dari system prompt-nya (src/agent/prompts/lead.md).
const isLeadRequest = (request) =>
  request.messages?.[0]?.role === 'system' && String(request.messages[0].content).startsWith('Anda Lead Agent');

// Mengembalikan isi pesan assistant (string JSON) untuk request Lead.
function leadReply(request) {
  const brief = JSON.parse(request.messages[1].content);
  const name = scenarioOf(brief.objective);
  if (!builders[name]) throw new Error(`unknown Lead scenario: ${name}`);
  return JSON.stringify(builders[name](brief));
}

module.exports = { isLeadRequest, leadReply, scenarioOf, scenarios: Object.keys(builders) };
