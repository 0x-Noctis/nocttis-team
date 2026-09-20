export const capabilityKeys = ['chat', 'streaming', 'tools', 'parallel_tools'] as const;
export const MAX_SAFE_INTEGER = Number.MAX_SAFE_INTEGER;

export function isPositiveSafeInteger(value: number): boolean {
  return Number.isSafeInteger(value) && value > 0;
}

export function isNonNegativeSafeInteger(value: number): boolean {
  return Number.isSafeInteger(value) && value >= 0;
}

export type CapabilityKey = (typeof capabilityKeys)[number];
export type VerifiedCapability = 'unknown' | 'supported' | 'unsupported';
export type ProbeKind = 'chat' | 'streaming' | 'tools';
export type ProbeStatus = 'succeeded' | 'failed';
export type ProbeErrorCode =
  | 'authentication_failed'
  | 'rate_limited'
  | 'timeout'
  | 'invalid_response'
  | 'provider_unavailable'
  | 'context_too_large';

export type ClaimedCapabilities = Record<CapabilityKey, boolean>;
export type VerifiedCapabilities = Record<CapabilityKey, VerifiedCapability>;

export interface ModelCapabilities {
  claimed: ClaimedCapabilities;
  verified: VerifiedCapabilities;
}

export interface ProviderInput {
  id: string;
  base_url: string;
  api_key_env: string;
  request_timeout_seconds: number;
}

export interface ProviderResponse extends ProviderInput {
  secret_configured: boolean;
}

export interface ModelInput {
  id: string;
  provider_id: string;
  remote_name: string;
  class: string;
  context_window: number;
  max_output_tokens: number;
  claimed_capabilities: ClaimedCapabilities;
}

export interface ModelResponse {
  id: string;
  provider_id: string;
  remote_name: string;
  class: string;
  context_window: number;
  max_output_tokens: number;
  capabilities: ModelCapabilities;
}

export interface ProbeResult {
  provider_id: string;
  model_id: string;
  kind: ProbeKind;
  status: ProbeStatus;
  verified: VerifiedCapability;
  latency_ms: number;
  error_code: ProbeErrorCode | null;
}

export interface ProbeResponse {
  result: ProbeResult;
}

export interface ApiError {
  error: {
    code: string;
    message: string;
    details: Record<string, unknown>;
    request_id: string;
  };
}

export type ProviderViewState =
  | { status: 'loading' }
  | { status: 'empty' }
  | { status: 'error'; error: ApiError }
  | {
      status: 'success';
      provider: ProviderResponse;
      models: ModelResponse[];
      probes: ProbeResult[];
    };

export const taskStatuses = [
  'DRAFT',
  'PLANNED',
  'READY',
  'ASSIGNED',
  'RUNNING',
  'SELF_CHECK',
  'REVIEW',
  'CHANGES_REQUESTED',
  'VERIFY',
  'FAILED',
  'INTEGRATE',
  'CONFLICT',
  'NEEDS_HUMAN',
  'DONE',
  'CANCELLED'
] as const;

export type TaskStatus = (typeof taskStatuses)[number];

export interface TaskLimits {
  max_input_tokens: number;
  max_output_tokens: number;
  max_tool_calls: number;
  max_attempts: number;
  timeout_seconds: number;
}

export function isValidTaskLimits(limits: TaskLimits): boolean {
  return (
    isPositiveSafeInteger(limits.max_input_tokens) &&
    isPositiveSafeInteger(limits.max_output_tokens) &&
    isPositiveSafeInteger(limits.max_tool_calls) &&
    isPositiveSafeInteger(limits.max_attempts) &&
    limits.max_attempts <= 10 &&
    isPositiveSafeInteger(limits.timeout_seconds)
  );
}

export interface TaskContract {
  id: string;
  project_id: string;
  project_run_id: string;
  title: string;
  role: string;
  objective: string;
  depends_on: string[];
  allowed_paths: string[];
  context_refs: string[];
  acceptance_criteria: string[];
  verification_commands: string[];
  limits: TaskLimits;
}

export interface TaskView {
  contract: TaskContract;
  status: TaskStatus;
}

export interface TaskTimelineEvent {
  id: string;
  status: TaskStatus;
  label: string;
  timestamp: string;
  detail: string;
}

export type ArtifactKind = 'patch' | 'log' | 'report' | 'test_result';

export interface TaskArtifact {
  id: string;
  name: string;
  kind: ArtifactKind;
  size_bytes: number;
  download_url: string;
}

export type DiffLineKind = 'context' | 'addition' | 'deletion';

export interface DiffLine {
  kind: DiffLineKind;
  old_line: number | null;
  new_line: number | null;
  content: string;
}

export interface TaskDiff {
  file: string;
  lines: DiffLine[];
}

export interface TaskUsage {
  input_tokens: number;
  cached_tokens: number;
  output_tokens: number;
  total_tokens: number;
  estimated: boolean;
}

export type TaskViewState =
  | { state: 'loading' }
  | { state: 'empty' }
  | { state: 'error'; message: string }
  | {
      state: 'ready';
      task: TaskView;
      timeline: TaskTimelineEvent[];
      artifacts: TaskArtifact[];
      diff: TaskDiff | null;
      usage: TaskUsage;
    };
