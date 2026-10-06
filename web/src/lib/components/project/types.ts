import type { TaskContract } from '$lib/api/types';

// Tipe UI untuk kontrak project/run/plan (cermin src/api/contracts/{project,plan}.rs).
// Ditaruh di sini, bukan di api/types.ts, karena api/types.ts di luar Allowed Paths M3-004.

export const runStatuses = ['PLANNING', 'AWAITING_APPROVAL', 'RUNNING', 'PAUSED', 'DONE', 'CANCELLED', 'FAILED'] as const;
export type RunStatus = (typeof runStatuses)[number];
export type PlanStatus = 'PROPOSED' | 'APPROVED' | 'REJECTED';
export type ApprovalDecision = 'APPROVED' | 'REJECTED';

export interface ProjectInput {
  id: string;
  name: string;
  repository_path: string;
}

export interface ProjectRunInput {
  id: string;
  project_id: string;
  objective: string;
  acceptance_criteria: string[];
  token_budget: number;
}

export interface ProjectRunView extends ProjectRunInput {
  status: RunStatus;
}

export interface ProposedPlan {
  id: string;
  project_run_id: string;
  version: number;
  tasks: TaskContract[];
  risk_flags: string[];
  status: PlanStatus;
}

export interface PlanApprovalInput {
  plan_id: string;
  actor_id: string;
  decision: ApprovalDecision;
  reason: string | null;
}

// Pemakaian token run: used = sudah dibelanjakan, reserved = dipesan untuk task berjalan.
export interface RunBudget {
  limit: number;
  used: number;
  reserved: number;
  estimated: boolean;
}

export type ProjectViewState =
  | { state: 'loading' }
  | { state: 'empty' }
  | { state: 'error'; message: string }
  | { state: 'ready'; run: ProjectRunView; plan: ProposedPlan | null; budget: RunBudget };

// Ambang peringatan budget: 70% warning, 85% checkpoint, 100% stop (rancangan M4-003).
export type BudgetLevel = 'ok' | 'warning' | 'checkpoint' | 'exhausted';

export function budgetLevel(percent: number): BudgetLevel {
  if (percent >= 100) return 'exhausted';
  if (percent >= 85) return 'checkpoint';
  if (percent >= 70) return 'warning';
  return 'ok';
}

// Token terburuk satu plan: tiap task boleh memakai (input + output) per attempt.
// Mengembalikan null bila hasilnya bukan safe integer supaya UI menampilkan "Invalid", bukan angka salah.
export function worstCaseTokens(tasks: TaskContract[]): number | null {
  let total = 0;
  for (const { limits } of tasks) {
    total += (limits.max_input_tokens + limits.max_output_tokens) * limits.max_attempts;
    if (!Number.isSafeInteger(total)) return null;
  }
  return total;
}

// Pisahkan criteria per baris; baris kosong dibuang (sama seperti backend menolak string kosong).
export function parseCriteria(text: string): string[] {
  return text.split('\n').map((line) => line.trim()).filter(Boolean);
}
