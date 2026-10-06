import type { TaskStatus, TaskView } from '$lib/api/types';

// Logika murni board/DAG: tanpa DOM supaya bisa dites dengan `node graph.test.mjs`.

export type BoardColumn = 'blocked' | 'queued' | 'in_progress' | 'checking' | 'attention' | 'closed';

export interface Blocker {
  id: string;
  // Status dependency, atau MISSING bila task itu tidak ada di plan.
  status: TaskStatus | 'MISSING';
}

export function indexTasks(tasks: TaskView[]): Map<string, TaskView> {
  return new Map(tasks.map((task) => [task.contract.id, task]));
}

// Dependency yang belum DONE. Cancelled/failed tetap dihitung: task tidak akan pernah siap.
export function blockedBy(task: TaskView, byId: Map<string, TaskView>): Blocker[] {
  return task.contract.depends_on.flatMap((id): Blocker[] => {
    const dependency = byId.get(id);
    if (!dependency) return [{ id, status: 'MISSING' }];
    return dependency.status === 'DONE' ? [] : [{ id, status: dependency.status }];
  });
}

const columnByStatus: Partial<Record<TaskStatus, BoardColumn>> = {
  ASSIGNED: 'in_progress',
  RUNNING: 'in_progress',
  SELF_CHECK: 'checking',
  REVIEW: 'checking',
  VERIFY: 'checking',
  INTEGRATE: 'checking',
  CHANGES_REQUESTED: 'attention',
  FAILED: 'attention',
  CONFLICT: 'attention',
  NEEDS_HUMAN: 'attention',
  DONE: 'closed',
  CANCELLED: 'closed'
};

export function columnOf(task: TaskView, byId: Map<string, TaskView>): BoardColumn {
  return columnByStatus[task.status] ?? (blockedBy(task, byId).length ? 'blocked' : 'queued');
}

// Kelompokkan task per kedalaman dependency (level 0 = tanpa dependency).
// Task yang dependency-nya hilang atau melingkar masuk `unresolved`; urutan stabil menurut id.
export function dependencyLevels(tasks: TaskView[]): { levels: TaskView[][]; unresolved: TaskView[] } {
  const byId = indexTasks(tasks);
  const placed = new Set<string>();
  const levels: TaskView[][] = [];
  let remaining = [...tasks].sort((a, b) => a.contract.id.localeCompare(b.contract.id));
  while (remaining.length) {
    const level = remaining.filter((task) => task.contract.depends_on.every((id) => placed.has(id) && byId.has(id)));
    if (!level.length) break;
    levels.push(level);
    for (const task of level) placed.add(task.contract.id);
    remaining = remaining.filter((task) => !placed.has(task.contract.id));
  }
  return { levels, unresolved: remaining };
}
