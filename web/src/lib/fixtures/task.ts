import type {
  TaskArtifact,
  TaskContract,
  TaskDiff,
  TaskTimelineEvent,
  TaskUsage,
  TaskView,
  TaskViewState
} from '$lib/api/types';

export const activeTask: TaskContract = {
  id: 'M2-004',
  project_id: 'NOCTIS',
  title: 'Web task components berbasis fixture',
  role: 'frontend_engineer',
  objective: 'Build presentational task components without API wiring.',
  depends_on: ['M1 exit gate'],
  allowed_paths: ['web/src/lib/components/task/**', 'web/src/lib/fixtures/task.ts'],
  context_refs: ['artifact://decisions/task-contract-v1'],
  acceptance_criteria: ['Status includes text and icon', 'Diff remains keyboard readable'],
  verification_commands: ['npm run check', 'npm run build'],
  limits: {
    max_input_tokens: 30000,
    max_output_tokens: 8000,
    max_tool_calls: 40,
    max_attempts: 2,
    timeout_seconds: 1200
  }
};

export const activeTaskView: TaskView = { contract: activeTask, status: 'RUNNING' };
export const completedTaskView: TaskView = { contract: { ...activeTask, id: 'M2-003', title: 'Git worktree manager' }, status: 'DONE' };
export const failedTaskView: TaskView = { contract: { ...activeTask, id: 'M2-002', title: 'Artifact store filesystem' }, status: 'FAILED' };
export const changesRequestedTaskView: TaskView = { contract: activeTask, status: 'CHANGES_REQUESTED' };

export const taskTimeline: TaskTimelineEvent[] = [
  { id: 'event-1', status: 'READY', label: 'Contract ready', timestamp: '2026-09-20T08:00:00Z', detail: 'Dependencies and task contract validated.' },
  { id: 'event-2', status: 'ASSIGNED', label: 'Assigned to worker', timestamp: '2026-09-20T08:02:00Z', detail: 'Scoped worktree and branch reserved.' },
  { id: 'event-3', status: 'RUNNING', label: 'Implementation started', timestamp: '2026-09-20T08:05:00Z', detail: 'Worker is editing allowed paths.' }
];

export const taskArtifacts: TaskArtifact[] = [
  { id: 'artifact-1', name: 'task-components.patch', kind: 'patch', size_bytes: 18420, download_url: '/artifacts/artifact-1' },
  { id: 'artifact-2', name: 'npm-check.txt', kind: 'test_result', size_bytes: 2048, download_url: '/artifacts/artifact-2' }
];

export const taskDiff: TaskDiff = {
  file: 'web/src/lib/components/task/TaskStatusBadge.svelte',
  lines: [
    { kind: 'context', old_line: 1, new_line: 1, content: '<script lang="ts">' },
    { kind: 'deletion', old_line: 2, new_line: null, content: "  let label = 'Running';" },
    { kind: 'addition', old_line: null, new_line: 2, content: "  let status: TaskStatus = 'RUNNING';" }
  ]
};

export const measuredUsage: TaskUsage = { input_tokens: 8400, cached_tokens: 3100, output_tokens: 1900, total_tokens: 10300, estimated: false };
export const estimatedUsage: TaskUsage = { input_tokens: 8000, cached_tokens: 0, output_tokens: 2000, total_tokens: 10000, estimated: true };

export const loadingTaskState: TaskViewState = { state: 'loading' };
export const emptyTaskState: TaskViewState = { state: 'empty' };
export const errorTaskState: TaskViewState = { state: 'error', message: 'Task data could not be loaded.' };
export const activeTaskState: TaskViewState = { state: 'ready', task: activeTaskView, timeline: taskTimeline, artifacts: taskArtifacts, diff: taskDiff, usage: estimatedUsage };
export const completedTaskState: TaskViewState = { state: 'ready', task: completedTaskView, timeline: taskTimeline, artifacts: taskArtifacts, diff: taskDiff, usage: measuredUsage };
export const failedTaskState: TaskViewState = { state: 'ready', task: failedTaskView, timeline: taskTimeline, artifacts: taskArtifacts, diff: taskDiff, usage: measuredUsage };
export const changesRequestedTaskState: TaskViewState = { state: 'ready', task: changesRequestedTaskView, timeline: taskTimeline, artifacts: taskArtifacts, diff: taskDiff, usage: estimatedUsage };
