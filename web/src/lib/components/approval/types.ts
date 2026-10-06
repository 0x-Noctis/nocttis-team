// Tipe tampilan antrean keputusan manusia (M4-009); cermin GET /api/v1/approvals.

export interface PendingPlan {
  plan_id: string;
  run_id: string;
  project_name: string;
  run_objective: string;
  run_status: string;
  version: number;
  task_count: number;
  risk_flags: string[];
  reserved_tokens: number;
  available_tokens: number;
  over_budget: boolean;
  created_at: string;
}

export interface ContextEvent {
  id: number;
  event_type: string;
  actor: string;
  actor_id: string | null;
  from_status: string | null;
  to_status: string | null;
  payload: Record<string, unknown>;
  created_at: string;
}

export interface AttentionTask {
  task_id: string;
  title: string;
  status: 'NEEDS_HUMAN' | 'CONFLICT';
  /** Dikirim kembali sebagai expected_version; versi basi ditolak server (409). */
  version: number;
  run_id: string;
  project_name: string;
  run_objective: string;
  updated_at: string;
  last_error: string | null;
  has_patch: boolean;
  can_retry: boolean;
  recent_events: ContextEvent[];
}

export interface PlanDecision {
  plan_id: string;
  run_id: string;
  version: number;
  outcome: 'APPROVED' | 'REJECTED';
  actor_id: string;
  reason: string | null;
}

export interface TaskDecision {
  event_id: number;
  task_id: string;
  from_status: string;
  to_status: string;
  actor_id: string;
  reason: string | null;
  created_at: string;
}

export interface ApprovalsView {
  pending_plans: PendingPlan[];
  attention_tasks: AttentionTask[];
  plan_decisions: PlanDecision[];
  task_decisions: TaskDecision[];
}

export type ApprovalsViewState =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'ready'; view: ApprovalsView };
