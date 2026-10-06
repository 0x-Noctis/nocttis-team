<script lang="ts">
  import type { PlanDecision, TaskDecision } from './types';

  let { plans, tasks }: { plans: PlanDecision[]; tasks: TaskDecision[] } = $props();
</script>

<section aria-labelledby="history-title">
  <h2 id="history-title">Recent decisions</h2>
  {#if plans.length === 0 && tasks.length === 0}<p>No decisions have been recorded yet.</p>
  {:else}
    <table><caption class="sr">Who decided what</caption>
      <thead><tr><th scope="col">Decision</th><th scope="col">Subject</th><th scope="col">Decided by</th><th scope="col">Reason</th></tr></thead>
      <tbody>
        {#each plans as decision (decision.plan_id)}
          <tr><td>{decision.outcome === 'APPROVED' ? '✓ Plan approved' : '× Plan rejected'}</td><td><a href={`/runs/${encodeURIComponent(decision.run_id)}`}>{decision.plan_id}</a> (v{decision.version})</td><td>{decision.actor_id}</td><td>{decision.reason ?? '—'}</td></tr>
        {/each}
        {#each tasks as decision (decision.event_id)}
          <tr><td>{decision.to_status === 'CANCELLED' ? '× Task cancelled' : '↺ Task retried'}</td><td><a href={`/tasks/${encodeURIComponent(decision.task_id)}`}>{decision.task_id}</a> ({decision.from_status} → {decision.to_status})</td><td>{decision.actor_id}</td><td>{decision.reason ?? '—'}</td></tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>

<style>
  section { display: grid; gap: .75rem; padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; overflow-x: auto; } h2 { margin: 0; }
  table { width: 100%; min-width: 34rem; border-collapse: collapse; } th, td { padding: .45rem .55rem; border-bottom: 1px solid #394139; text-align: left; } th { color: #aeb5ad; font-size: .8rem; } a { color: #90e0a8; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); } a:focus-visible { outline: 3px solid #f4d35e; outline-offset: 3px; }
</style>
