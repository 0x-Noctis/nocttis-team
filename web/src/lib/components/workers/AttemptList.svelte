<script lang="ts">
  import type { AttemptRecord, AttemptStatus } from './types';

  let { attempts, loading = false, error = '' }: { attempts: AttemptRecord[]; loading?: boolean; error?: string } = $props();

  const statuses: Record<AttemptStatus, { icon: string; label: string }> = {
    assigned: { icon: '→', label: 'Assigned' },
    running: { icon: '▶', label: 'Running' },
    completed: { icon: '✓', label: 'Completed' },
    failed: { icon: '×', label: 'Failed' },
    recovery_required: { icon: '!', label: 'Needs recovery' }
  };
</script>

<section aria-labelledby="attempts-title">
  <h2 id="attempts-title">Attempts</h2>
  {#if loading}<p aria-live="polite" aria-busy="true">Loading attempts…</p>
  {:else if error}<p class="bad" role="alert">Attempts unavailable: {error}</p>
  {:else if attempts.length === 0}<p>No attempts yet.</p>
  {:else}
    <table><caption class="sr">Attempts per task</caption>
      <thead><tr><th scope="col">Task</th><th scope="col">Attempt</th><th scope="col">Status</th><th scope="col">Tokens</th><th scope="col">Error</th></tr></thead>
      <tbody>{#each attempts as attempt (attempt.task_id + attempt.number)}
        <tr><td>{attempt.task_id}</td><td>{attempt.number}</td><td><span aria-hidden="true">{statuses[attempt.status].icon}</span> {statuses[attempt.status].label}</td><td>{attempt.tokens.toLocaleString()}</td><td>{#if attempt.error_code}<code>{attempt.error_code}</code>{:else}—{/if}</td></tr>
      {/each}</tbody></table>
  {/if}
</section>

<style>
  section { display: grid; gap: .75rem; padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; overflow-x: auto; } h2 { margin: 0; }
  table { width: 100%; border-collapse: collapse; min-width: 28rem; } th, td { padding: .4rem .5rem; border-bottom: 1px solid #394139; text-align: left; } th { color: #aeb5ad; font-size: .8rem; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); } .bad { color: #ffb0b0; }
</style>
