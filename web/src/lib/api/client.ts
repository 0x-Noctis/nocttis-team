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
import type {
  PlanApprovalInput,
  ProjectInput,
  ProjectRunInput,
  ProjectRunView,
  ProposedPlan,
  RunBudget,
  RunStatus
} from '$lib/components/project/types';

export class ApiRequestError extends Error {
  constructor(public readonly envelope: ApiError) {
    super(envelope.error.message);
  }
}

interface Page<T> {
  items: T[];
  next_cursor: string | null;
}

// Ubah error apa pun (envelope API atau jaringan) menjadi bentuk envelope agar bisa ditampilkan seragam.
export function asApiError(reason: unknown): ApiError {
  if (reason instanceof ApiRequestError) return reason.envelope;
  return {
    error: {
      code: 'NETWORK_ERROR',
      message: reason instanceof Error ? reason.message : 'Request failed.',
      details: {},
      request_id: 'unavailable'
    }
  };
}

// Backend menolak ID project/run yang bukan UUID kanonik (huruf kecil, bertanda hubung).
export const isCanonicalUuid = (value: string) =>
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);

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

export interface RepositoryMap {
  files: string[];
  languages: string[];
  frameworks: string[];
  entry_points: string[];
  test_commands: string[];
  config_files: string[];
  instruction_files: string[];
  truncated: boolean;
}

export interface RunDetail {
  run: ProjectRunView;
  budget: RunBudget;
}

const enc = encodeURIComponent;

export const projectApi = {
  list: () => request<{ items: ProjectInput[] }>('/api/v1/projects').then(({ items }) => items),
  get: (projectId: string) =>
    request<{ project: ProjectInput }>(`/api/v1/projects/${enc(projectId)}`).then(({ project }) => project),
  create: (project: ProjectInput) =>
    request<{ project: ProjectInput }>('/api/v1/projects', mutation('POST', project)).then(({ project }) => project),
  discover: (projectId: string) =>
    request<{ repository_map: RepositoryMap }>(`/api/v1/projects/${enc(projectId)}/discover`, mutation('POST', {})).then(
      ({ repository_map }) => repository_map
    ),
  runs: (projectId: string) =>
    request<{ items: ProjectRunView[] }>(`/api/v1/projects/${enc(projectId)}/runs`).then(({ items }) => items),
  createRun: (projectId: string, run: ProjectRunInput) =>
    request<{ run: ProjectRunView }>(`/api/v1/projects/${enc(projectId)}/runs`, mutation('POST', run)).then(({ run }) => run)
};

const MAX_TASK_PAGES = 10;

export const runApi = {
  get: (runId: string) => request<RunDetail>(`/api/v1/runs/${enc(runId)}`),
  plans: (runId: string) =>
    request<{ items: ProposedPlan[] }>(`/api/v1/runs/${enc(runId)}/plans`).then(({ items }) => items),
  // Task milik satu run; mengikuti cursor sampai habis (maks MAX_TASK_PAGES halaman supaya tidak tak terbatas).
  tasks: async (runId: string): Promise<TaskResponse[]> => {
    const tasks: TaskResponse[] = [];
    let cursor: string | null = null;
    for (let page = 0; page < MAX_TASK_PAGES; page += 1) {
      const query: string = `project_run_id=${enc(runId)}&limit=100${cursor ? `&cursor=${enc(cursor)}` : ''}`;
      const result: Page<TaskResponse> = await request<Page<TaskResponse>>(`/api/v1/tasks?${query}`);
      tasks.push(...result.items);
      cursor = result.next_cursor;
      if (!cursor) break;
    }
    return tasks;
  },
  // Meminta Lead Agent menyusun plan (bisa memakan waktu sebesar timeout provider); hasilnya plan PROPOSED.
  leadPlan: (runId: string, modelId?: string) =>
    request<{ plan: ProposedPlan }>(
      `/api/v1/runs/${enc(runId)}/lead-plan`,
      mutation('POST', modelId ? { model_id: modelId } : {})
    ).then(({ plan }) => plan),
  decide: (runId: string, approval: PlanApprovalInput) =>
    request<{ plan: ProposedPlan }>(
      `/api/v1/runs/${enc(runId)}/${approval.decision === 'APPROVED' ? 'approve' : 'reject'}-plan`,
      mutation('POST', approval)
    ).then(({ plan }) => plan),
  // expected_status melindungi dari aksi basi: server menolak (409) bila status run sudah berubah.
  transition: (runId: string, action: 'pause' | 'resume' | 'cancel', expectedStatus: RunStatus) =>
    request<{ run: ProjectRunView }>(`/api/v1/runs/${enc(runId)}/${action}`, mutation('POST', { expected_status: expectedStatus })).then(
      ({ run }) => run
    )
};
