// Bentuk respons GET /api/v1/operations dan /operations/config (src/api/operations.rs).
export type ComponentState = 'ok' | 'degraded' | 'down';
export type ComponentName = 'database' | 'migrations' | 'workers' | 'providers';

export interface CostView {
  tracked: boolean;
  micros: number;
  priced_rows: number;
  total_rows: number;
}

export interface UsageView {
  input_tokens: number;
  output_tokens: number;
  // Bagian total yang berupa estimasi, bukan angka dari provider.
  estimated_tokens: number;
  cost?: CostView;
}

export interface ProviderModelView {
  id: string;
  class: string | null;
  tools: 'supported' | 'unsupported' | 'unknown';
}

export interface ProviderReadinessView {
  id: string;
  host: string;
  secret_configured: boolean;
  models: ProviderModelView[];
}

export interface RetentionActionView {
  at: string;
  dry_run: boolean;
  kind: 'orphan_artifact' | 'integration_worktree';
  target: string;
  outcome: 'deleted' | 'would_delete' | 'failed';
  detail: string | null;
}

export interface RetentionView {
  last_run_at: string | null;
  last_24h: { deleted: number; failed: number };
  recent: RetentionActionView[];
}

export interface OperationsView {
  ready: boolean;
  components: Record<ComponentName, ComponentState>;
  tasks: Record<string, number> | null;
  attempts: Record<string, number> | null;
  stale_attempts: number | null;
  usage: UsageView | null;
  providers: ProviderReadinessView[] | null;
  retention: RetentionView | null;
}

export type ConfigView = Record<string, Record<string, unknown>>;
