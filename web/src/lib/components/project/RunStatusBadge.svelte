<script lang="ts">
  import type { RunStatus } from './types';

  let { status }: { status: RunStatus } = $props();

  // Ikon + teks supaya status tidak bergantung pada warna.
  const presentation: Record<RunStatus, { icon: string; label: string; tone: string }> = {
    PLANNING: { icon: '◇', label: 'Planning', tone: 'neutral' },
    AWAITING_APPROVAL: { icon: '?', label: 'Awaiting approval', tone: 'warning' },
    RUNNING: { icon: '▶', label: 'Running', tone: 'active' },
    PAUSED: { icon: '‖', label: 'Paused', tone: 'warning' },
    DONE: { icon: '✓', label: 'Done', tone: 'positive' },
    CANCELLED: { icon: '−', label: 'Cancelled', tone: 'neutral' },
    FAILED: { icon: '×', label: 'Failed', tone: 'danger' }
  };
</script>

<span class="badge {presentation[status].tone}" aria-label={`Run status: ${presentation[status].label}`}>
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
