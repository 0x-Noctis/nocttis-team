<script lang="ts">
  import type { TaskStatus } from '$lib/api/types';

  let { status }: { status: TaskStatus } = $props();

  const presentation: Record<TaskStatus, { icon: string; label: string; tone: string }> = {
    DRAFT: { icon: '○', label: 'Draft', tone: 'neutral' },
    PLANNED: { icon: '◇', label: 'Planned', tone: 'neutral' },
    READY: { icon: '✓', label: 'Ready', tone: 'positive' },
    ASSIGNED: { icon: '→', label: 'Assigned', tone: 'active' },
    RUNNING: { icon: '▶', label: 'Running', tone: 'active' },
    SELF_CHECK: { icon: '⌕', label: 'Self check', tone: 'active' },
    REVIEW: { icon: '◉', label: 'Review', tone: 'active' },
    CHANGES_REQUESTED: { icon: '↺', label: 'Changes requested', tone: 'warning' },
    VERIFY: { icon: '◆', label: 'Verify', tone: 'active' },
    FAILED: { icon: '×', label: 'Failed', tone: 'danger' },
    INTEGRATE: { icon: '⇄', label: 'Integrate', tone: 'active' },
    CONFLICT: { icon: '!', label: 'Conflict', tone: 'danger' },
    NEEDS_HUMAN: { icon: '?', label: 'Needs human', tone: 'warning' },
    DONE: { icon: '✓', label: 'Done', tone: 'positive' },
    CANCELLED: { icon: '−', label: 'Cancelled', tone: 'neutral' }
  };
</script>

<span class="badge {presentation[status].tone}" aria-label={`Status: ${presentation[status].label}`}>
  <span aria-hidden="true">{presentation[status].icon}</span>
  {presentation[status].label}
</span>

<style>
  .badge { display: inline-flex; align-items: center; gap: .4rem; width: fit-content; padding: .3rem .55rem; border: 1px solid #697169; background: #202420; color: #eef2ec; font: 700 .78rem/1.2 ui-monospace, monospace; text-transform: uppercase; }
  .positive { border-color: #65a978; background: #102217; }
  .active { border-color: #7aa7d8; background: #101d29; }
  .warning { border-color: #d8c98a; background: #292612; }
  .danger { border-color: #ff8b8b; background: #291313; }
</style>
