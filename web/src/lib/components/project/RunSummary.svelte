<script lang="ts">
  import RunStatusBadge from './RunStatusBadge.svelte';
  import type { ProjectRunView } from './types';

  let { run, loading = false, error = '' }: { run?: ProjectRunView; loading?: boolean; error?: string } = $props();
</script>

{#if loading}
  <section class="state" aria-live="polite" aria-busy="true"><h2>Loading run</h2><p>Reading run objective…</p></section>
{:else if error}
  <section class="state error" role="alert"><h2>Run unavailable</h2><p>{error}</p></section>
{:else if !run}
  <section class="state"><h2>No run selected</h2><p>Create a run to start planning.</p></section>
{:else}
  <article aria-labelledby={`run-${run.id}`}>
    <header><div><p class="eyebrow">{run.project_id} · {run.id}</p><h2 id={`run-${run.id}`}>{run.objective}</h2></div><RunStatusBadge status={run.status} /></header>
    <h3>Acceptance criteria</h3>
    <ul>{#each run.acceptance_criteria as criterion}<li>{criterion}</li>{/each}</ul>
  </article>
{/if}

<style>
  article, .state { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  header { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 1rem; } h2 { margin: 0; font-size: 1.3rem; } h3 { margin-bottom: .4rem; }
  .eyebrow { margin: 0 0 .35rem; color: #90e0a8; font: 700 .75rem/1.2 ui-monospace, monospace; letter-spacing: .12em; text-transform: uppercase; }
  .error { border-color: #ff8b8b; color: #ffb0b0; }
</style>
