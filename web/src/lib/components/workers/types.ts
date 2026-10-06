import type { TaskStatus } from '$lib/api/types';

// Tipe tampilan dashboard paralel (M4-004). Bentuk disesuaikan dengan store M4-001..003:
// slot = attempt aktif (agent_runs), queue = task READY yang belum diklaim, lease = file_leases,
// gauge = BudgetStore::run_view/posisi attempt. Dipisah dari api/types.ts karena di luar Allowed Paths.

export type SlotState = 'idle' | 'running' | 'pausing' | 'paused' | 'cancelling' | 'draining';

export interface SlotLease {
  pattern: string;
  expires_in_seconds: number;
}

export interface GaugeValue {
  label: string;
  /** Batas penuh scope (untuk run: token_budget). */
  limit: number;
  used: number;
  /** Token yang ditahan untuk request yang sedang berjalan. */
  held: number;
  estimated: boolean;
  /** Bagian limit yang hanya untuk recovery/integrasi; 0 untuk scope selain run. */
  reserve: number;
}

export interface WorkerSlot {
  slot: number;
  state: SlotState;
  task_id?: string;
  title?: string;
  task_status?: TaskStatus;
  attempt?: { number: number; max: number; branch: string };
  heartbeat_age_seconds?: number;
  leases: SlotLease[];
  tokens?: GaugeValue;
}

export type QueueReason = 'slot' | 'dependency' | 'lease' | 'budget' | 'paused' | 'attempts';

/** Lease aktif milik satu task di dalam run (tampilan tingkat run). */
export interface RunLease extends SlotLease {
  task_id: string;
}

export interface QueueItem {
  task_id: string;
  title: string;
  priority: number;
  reason: QueueReason;
  /** Penjelasan yang terbaca manusia, mis. "Waiting for task-a (running)". */
  detail: string;
}

export interface LeaseConflict {
  task_id: string;
  pattern: string;
  holder_task_id: string;
}

export type AttemptStatus = 'assigned' | 'running' | 'completed' | 'failed' | 'recovery_required';

export interface AttemptRecord {
  task_id: string;
  number: number;
  status: AttemptStatus;
  error_code: string | null;
  tokens: number;
}

export type ParallelViewState =
  | { state: 'loading' }
  | { state: 'empty' }
  | { state: 'error'; message: string }
  | { state: 'ready'; slots: WorkerSlot[]; queue: QueueItem[]; leases: RunLease[]; conflicts: LeaseConflict[]; attempts: AttemptRecord[]; run: GaugeValue };
