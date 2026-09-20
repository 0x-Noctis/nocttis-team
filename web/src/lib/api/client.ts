import type {
  ApiError,
  ModelInput,
  ModelResponse,
  ProbeKind,
  ProbeResponse,
  ProviderInput,
  ProviderResponse,
  TaskContract,
  TaskStatus
} from './types';

export class ApiRequestError extends Error {
  constructor(public readonly envelope: ApiError) {
    super(envelope.error.message);
  }
}

interface Page<T> {
  items: T[];
  next_cursor: string | null;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, init);
  const body: unknown = response.status === 204 ? undefined : await response.json();
  if (!response.ok) throw new ApiRequestError(body as ApiError);
  return body as T;
}

function mutation(method: string, body?: unknown): RequestInit {
  return {
    method,
    headers: {
      'Content-Type': 'application/json',
      'Idempotency-Key': crypto.randomUUID()
    },
    body: body === undefined ? undefined : JSON.stringify(body)
  };
}

export interface TaskResponse {
  contract: TaskContract;
  status: TaskStatus;
  version: number;
}

export interface TaskEventResponse {
  id: number;
  task_id: string;
  actor: string;
  event_type: string;
  from_status: TaskStatus | null;
  to_status: TaskStatus | null;
  payload: Record<string, unknown>;
}

export interface ArtifactResponse {
  id: string;
  kind: string;
  size_bytes: number;
  sha256: string;
}

export const taskApi = {
  list: () => request<Page<TaskResponse>>('/api/v1/tasks'),
  get: (taskId: string) => request<TaskResponse>(`/api/v1/tasks/${encodeURIComponent(taskId)}`),
  create: (contract: TaskContract) =>
    request<TaskResponse>('/api/v1/tasks', mutation('POST', contract)),
  action: (taskId: string, action: 'start' | 'cancel' | 'retry', expectedVersion: number) =>
    request<TaskResponse>(
      `/api/v1/tasks/${encodeURIComponent(taskId)}/${action}`,
      mutation('POST', { expected_version: expectedVersion })
    ),
  events: (taskId: string) =>
    request<Page<TaskEventResponse>>(`/api/v1/tasks/${encodeURIComponent(taskId)}/events`).then(
      ({ items }) => items
    ),
  artifacts: (taskId: string) =>
    request<Page<ArtifactResponse>>(`/api/v1/tasks/${encodeURIComponent(taskId)}/artifacts`).then(
      ({ items }) => items
    ),
  diff: async (taskId: string) => {
    const response = await fetch(`/api/v1/tasks/${encodeURIComponent(taskId)}/diff`);
    if (!response.ok) {
      const body = (await response.json()) as ApiError;
      throw new ApiRequestError(body);
    }
    return response.text();
  },
  artifactUrl: (taskId: string, artifactId: string) =>
    `/api/v1/tasks/${encodeURIComponent(taskId)}/artifacts/${encodeURIComponent(artifactId)}`
};

export const providerApi = {
  list: () => request<Page<ProviderResponse>>('/api/v1/providers').then(({ items }) => items),
  get: (providerId: string) =>
    request<ProviderResponse>(`/api/v1/providers/${encodeURIComponent(providerId)}`),
  create: (provider: ProviderInput) =>
    request<ProviderResponse>('/api/v1/providers', mutation('POST', provider)),
  update: (providerId: string, provider: ProviderInput) =>
    request<ProviderResponse>(`/api/v1/providers/${encodeURIComponent(providerId)}`, mutation('PUT', provider)),
  delete: (providerId: string) =>
    request<void>(`/api/v1/providers/${encodeURIComponent(providerId)}`, mutation('DELETE')),
  models: (providerId: string) =>
    request<Page<ModelResponse>>(`/api/v1/providers/${encodeURIComponent(providerId)}/models`).then(
      ({ items }) => items
    ),
  createModel: (providerId: string, model: ModelInput) =>
    request<ModelResponse>(`/api/v1/providers/${encodeURIComponent(providerId)}/models`, mutation('POST', model)),
  getModel: (modelId: string) =>
    request<ModelResponse>(`/api/v1/models/${encodeURIComponent(modelId)}`),
  updateModel: (modelId: string, model: ModelInput) =>
    request<ModelResponse>(`/api/v1/models/${encodeURIComponent(modelId)}`, mutation('PUT', model)),
  deleteModel: (modelId: string) =>
    request<void>(`/api/v1/models/${encodeURIComponent(modelId)}`, mutation('DELETE')),
  probe: (modelId: string, kind: ProbeKind) =>
    request<ProbeResponse>(
      `/api/v1/models/${encodeURIComponent(modelId)}/probes/${kind}`,
      mutation('POST')
    )
};
