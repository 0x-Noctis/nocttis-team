import type {
  ApiError,
  ModelResponse,
  ProbeResult,
  ProviderResponse,
  ProviderViewState
} from '$lib/api/types';

export const configuredProvider: ProviderResponse = {
  id: 'primary',
  base_url: 'https://api.example.com/v1',
  api_key_env: 'PRIMARY_API_KEY',
  secret_configured: true,
  request_timeout_seconds: 180
};

export const missingSecretProvider: ProviderResponse = {
  ...configuredProvider,
  id: 'backup',
  api_key_env: 'BACKUP_API_KEY',
  secret_configured: false
};

export const modelWithVerifiedStates: ModelResponse = {
  id: 'coding-large',
  provider_id: configuredProvider.id,
  remote_name: 'vendor/coding-large',
  class: 'coding',
  context_window: 131072,
  max_output_tokens: 16384,
  capabilities: {
    claimed: { chat: true, streaming: true, tools: true, parallel_tools: false },
    verified: {
      chat: 'supported',
      streaming: 'unsupported',
      tools: 'unknown',
      parallel_tools: 'unknown'
    }
  }
};

export const successfulProbe: ProbeResult = {
  provider_id: configuredProvider.id,
  model_id: modelWithVerifiedStates.id,
  kind: 'chat',
  status: 'succeeded',
  verified: 'supported',
  latency_ms: 284,
  error_code: null
};

export const failedProbe: ProbeResult = {
  provider_id: configuredProvider.id,
  model_id: modelWithVerifiedStates.id,
  kind: 'streaming',
  status: 'failed',
  verified: 'unsupported',
  latency_ms: 1200,
  error_code: 'provider_unavailable'
};

export const providerApiError: ApiError = {
  error: {
    code: 'PROVIDER_UNAVAILABLE',
    message: 'Provider tidak dapat dihubungi. Periksa endpoint dan coba lagi.',
    details: {},
    request_id: 'req_fixture_provider_error'
  }
};

export const invalidNumericApiError: ApiError = {
  error: {
    code: 'INVALID_PROVIDER_TIMEOUT',
    message: 'Request timeout must be a positive safe integer.',
    details: { field: 'request_timeout_seconds' },
    request_id: 'req_fixture_invalid_provider_timeout'
  }
};

export const emptyProviders: ProviderViewState = { status: 'empty' };
export const loadingProviders: ProviderViewState = { status: 'loading' };
export const configuredProviderState: ProviderViewState = {
  status: 'success',
  provider: configuredProvider,
  models: [modelWithVerifiedStates],
  probes: [successfulProbe, failedProbe]
};
export const missingSecretProviderState: ProviderViewState = {
  status: 'success',
  provider: missingSecretProvider,
  models: [],
  probes: []
};
export const errorProviders: ProviderViewState = { status: 'error', error: providerApiError };
export const invalidNumericProviders: ProviderViewState = {
  status: 'error',
  error: invalidNumericApiError
};
