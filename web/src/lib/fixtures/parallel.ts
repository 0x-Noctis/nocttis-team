import type {
  AttemptRecord,
  GaugeValue,
  LeaseConflict,
  ParallelViewState,
  QueueItem,
  RunLease,
  SlotLease,
  WorkerSlot
} from '$lib/components/workers/types';

const gauge = (label: string, limit: number, used: number, held = 0, reserve = 0, estimated = false): GaugeValue => ({ label, limit, used, held, reserve, estimated });

// Run: batas kerja = 200.000 - 15% reserve = 170.000. 130.000 + 5.000 = 79% -> Warning.
export const runGaugeWarning = gauge('Run RUN-M4', 200000, 130000, 5000, 30000);
export const runGaugeOk = gauge('Run RUN-M4', 200000, 40000, 0, 30000);
export const runGaugeCheckpoint = gauge('Run RUN-M4', 200000, 150000, 0, 30000, true);
export const runGaugeStop = gauge('Run RUN-M4', 200000, 170000, 0, 30000);
export const taskGauges: GaugeValue[] = [
  gauge('backend-search', 76000, 20000, 2000),
  gauge('frontend-search', 76000, 61000, 0),
  gauge('docs-update', 76000, 70000, 2000, 0, true)
];

const lease = (pattern: string, expires_in_seconds: number): SlotLease => ({ pattern, expires_in_seconds });

export const slotsFour: WorkerSlot[] = [
  {
    slot: 1, state: 'running', task_id: 'backend-search', title: 'Implement search endpoint', task_status: 'RUNNING',
    attempt: { number: 1, max: 2, branch: 'noctis-backend-search-a1' }, heartbeat_age_seconds: 4,
    leases: [lease('src/backend.js', 52), lease('src/products.js', 52)], tokens: taskGauges[0]
  },
  {
    slot: 2, state: 'running', task_id: 'frontend-search', title: 'Add search box', task_status: 'REVIEW',
    attempt: { number: 2, max: 2, branch: 'noctis-frontend-search-a2' }, heartbeat_age_seconds: 41,
    leases: [lease('src/frontend/**', 19)], tokens: taskGauges[1]
  },
  {
    slot: 3, state: 'pausing', task_id: 'docs-update', title: 'Update docs', task_status: 'RUNNING',
    attempt: { number: 1, max: 2, branch: 'noctis-docs-update-a1' }, heartbeat_age_seconds: 75,
    leases: [lease('docs/**', 8)], tokens: taskGauges[2]
  },
  { slot: 4, state: 'idle', leases: [] }
];

export const slotsTwo: WorkerSlot[] = [slotsFour[0], { slot: 2, state: 'idle', leases: [] }];
export const slotsCancelling: WorkerSlot[] = [{ ...slotsFour[0], state: 'cancelling' }, { ...slotsFour[1], state: 'paused' }, { slot: 3, state: 'idle', leases: [] }];
export const slotsDraining: WorkerSlot[] = slotsFour.map((slot) => (slot.task_id ? { ...slot, state: 'draining' as const } : slot));

export const queueItems: QueueItem[] = [
  { task_id: 'integration-test', title: 'Cover search with a test', priority: 5, reason: 'dependency', detail: 'Waiting for backend-search (running), frontend-search (review).' },
  { task_id: 'refactor-products', title: 'Refactor products module', priority: 3, reason: 'lease', detail: 'src/products.js is leased by backend-search.' },
  { task_id: 'perf-pass', title: 'Performance pass', priority: 2, reason: 'budget', detail: 'Needs 12,000 tokens; 9,000 remain before the reserve.' },
  { task_id: 'cleanup', title: 'Remove dead code', priority: 0, reason: 'slot', detail: 'All 3 worker slots are busy.' },
  { task_id: 'flaky-migration', title: 'Run migration', priority: 0, reason: 'attempts', detail: 'No attempts left (2 of 2 used).' }
];

export const queuePaused: QueueItem[] = queueItems.map((item) => ({ ...item, reason: 'paused' as const, detail: 'Resume the run to schedule this task.' }));

export const activeLeases: RunLease[] = [
  { pattern: 'src/backend.js', task_id: 'backend-search', expires_in_seconds: 52 },
  { pattern: 'src/products.js', task_id: 'backend-search', expires_in_seconds: 52 },
  { pattern: 'src/frontend/**', task_id: 'frontend-search', expires_in_seconds: 19 },
  { pattern: 'docs/**', task_id: 'docs-update', expires_in_seconds: 8 }
];
export const conflicts: LeaseConflict[] = [{ task_id: 'refactor-products', pattern: 'src/products.js', holder_task_id: 'backend-search' }];

export const attempts: AttemptRecord[] = [
  { task_id: 'contract', number: 1, status: 'completed', error_code: null, tokens: 8400 },
  { task_id: 'backend-search', number: 1, status: 'running', error_code: null, tokens: 22000 },
  { task_id: 'frontend-search', number: 1, status: 'failed', error_code: 'recovery.stale', tokens: 30000 },
  { task_id: 'frontend-search', number: 2, status: 'running', error_code: null, tokens: 61000 },
  { task_id: 'migration', number: 1, status: 'recovery_required', error_code: 'recovery.tool_in_progress', tokens: 4100 }
];

export const loadingParallelState: ParallelViewState = { state: 'loading' };
export const emptyParallelState: ParallelViewState = { state: 'empty' };
export const errorParallelState: ParallelViewState = { state: 'error', message: 'Scheduler status could not be loaded (request_id req-456).' };
export const readyParallelState: ParallelViewState = { state: 'ready', slots: slotsFour, queue: queueItems, leases: activeLeases, conflicts, attempts, run: runGaugeWarning };
