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
