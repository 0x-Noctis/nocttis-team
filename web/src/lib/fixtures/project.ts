import type { TaskContract, TaskLimits, TaskView } from '$lib/api/types';
import type {
  PlanApprovalInput,
  ProjectInput,
  ProjectRunView,
  ProjectViewState,
  ProposedPlan,
  RunBudget
} from '$lib/components/project/types';

export const project: ProjectInput = { id: 'NOCTIS', name: 'Noctis Team', repository_path: '/srv/repos/noctis-team' };

export const run: ProjectRunView = {
  id: 'RUN-M3',
  project_id: 'NOCTIS',
  objective: 'Add a product search endpoint and the matching search box.',
  acceptance_criteria: ['GET /products?q= filters by name', 'Search box updates the list', 'All existing tests pass'],
  token_budget: 200000,
  status: 'AWAITING_APPROVAL'
};

const limits: TaskLimits = { max_input_tokens: 30000, max_output_tokens: 8000, max_tool_calls: 40, max_attempts: 2, timeout_seconds: 1200 };

const contract = (id: string, title: string, depends_on: string[], allowed_paths: string[]): TaskContract => ({
  id,
  project_id: 'NOCTIS',
  project_run_id: 'RUN-M3',
  title,
  role: 'worker',
  objective: `${title} within the allowed paths only.`,
  depends_on,
  allowed_paths,
  context_refs: [],
  acceptance_criteria: [`${title} is complete`],
  verification_commands: ['npm test'],
  limits
});

// Diamond: api-contract -> (backend, frontend) -> integration-test
export const planTasks: TaskContract[] = [
  contract('api-contract', 'Define search API contract', [], ['docs/api.md']),
  contract('backend-search', 'Implement search endpoint', ['api-contract'], ['src/backend.js', 'src/products.js']),
  contract('frontend-search', 'Add search box', ['api-contract'], ['src/frontend.js']),
  contract('search-e2e', 'Cover search with an integration test', ['backend-search', 'frontend-search'], ['test/**'])
];

export const proposedPlan: ProposedPlan = {
  id: 'PLAN-1',
  project_run_id: 'RUN-M3',
  version: 1,
  tasks: planTasks,
  risk_flags: ['backend-search and frontend-search both touch the products response shape'],
  status: 'PROPOSED'
};

export const approvedPlan: ProposedPlan = { ...proposedPlan, status: 'APPROVED' };
export const rejectedPlan: ProposedPlan = { ...proposedPlan, status: 'REJECTED' };
export const noRiskPlan: ProposedPlan = { ...proposedPlan, risk_flags: [] };

// Worst case = 4 * (30000 + 8000) * 2 = 304000, melebihi budget 200000.
export const overBudgetPlan: ProposedPlan = proposedPlan;

export const approveInput: PlanApprovalInput = { plan_id: 'PLAN-1', actor_id: 'human', decision: 'APPROVED', reason: null };

export const boardTasks: TaskView[] = [
  { contract: planTasks[0], status: 'DONE' },
  { contract: planTasks[1], status: 'RUNNING' },
  { contract: planTasks[2], status: 'CHANGES_REQUESTED' },
  { contract: planTasks[3], status: 'PLANNED' }
];

export const allBlockedTasks: TaskView[] = planTasks.map((task, index) => ({ contract: task, status: index === 0 ? 'FAILED' : 'PLANNED' }));

// Plan rusak untuk menguji tampilan unresolved: siklus dan dependency hilang.
export const brokenGraphTasks: TaskView[] = [
  { contract: contract('a', 'Cycle A', ['b'], ['a/**']), status: 'PLANNED' },
  { contract: contract('b', 'Cycle B', ['a'], ['b/**']), status: 'PLANNED' },
  { contract: contract('c', 'Missing dependency', ['ghost'], ['c/**']), status: 'PLANNED' },
  { contract: contract('d', 'Healthy task', [], ['d/**']), status: 'READY' }
];

export const budgetOk: RunBudget = { limit: 200000, used: 40000, reserved: 30000, estimated: false };
export const budgetWarning: RunBudget = { limit: 200000, used: 120000, reserved: 30000, estimated: false };
export const budgetCheckpoint: RunBudget = { limit: 200000, used: 150000, reserved: 25000, estimated: true };
export const budgetExhausted: RunBudget = { limit: 200000, used: 190000, reserved: 20000, estimated: false };

export const loadingProjectState: ProjectViewState = { state: 'loading' };
export const emptyProjectState: ProjectViewState = { state: 'empty' };
export const errorProjectState: ProjectViewState = { state: 'error', message: 'Run could not be loaded (request_id req-123).' };
export const readyProjectState: ProjectViewState = { state: 'ready', run, plan: proposedPlan, budget: budgetOk };
