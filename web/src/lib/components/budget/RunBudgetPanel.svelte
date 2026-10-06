<script lang="ts">
  import TokenGauge from './TokenGauge.svelte';
  import type { GaugeValue } from '../workers/types';

  let { run, tasks = [], loading = false, error = '' }: { run?: GaugeValue; tasks?: GaugeValue[]; loading?: boolean; error?: string } = $props();
</script>

<section aria-labelledby="run-budget-title">
  <h2 id="run-budget-title">Token budget</h2>
  {#if loading}<p aria-live="polite" aria-busy="true">Loading budget…</p>
  {:else if error}<p class="bad" role="alert">Budget unavailable: {error}</p>
  {:else if !run}<p>No budget data for this run.</p>
  {:else}
    <TokenGauge gauge={run} />
    {#if tasks.length}
      <h3>Per task</h3>
      <ul>{#each tasks as gauge (gauge.label)}<li><TokenGauge {gauge} compact /></li>{/each}</ul>
    {/if}
  {/if}
</section>

<style>
  section { display: grid; gap: .75rem; padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; } h2, h3 { margin: 0; } ul { display: grid; grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr)); gap: .75rem; margin: 0; padding: 0; list-style: none; } li { padding: .75rem; border: 1px solid #394139; } .bad { color: #ffb0b0; }
</style>
