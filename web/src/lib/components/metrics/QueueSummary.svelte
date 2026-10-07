<script lang="ts">
  import { formatNumber, humanize } from './helpers';

  let { tasks, attempts, stale }: { tasks: Record<string, number>; attempts: Record<string, number>; stale: number | null } = $props();

  // Baris dengan jumlah nol tetap tampil: himpunan status tetap, jadi tidak ada yang "hilang" tanpa kabar.
  const taskRows = $derived(Object.entries(tasks));
  const attemptRows = $derived(Object.entries(attempts));
</script>

<section aria-labelledby="queue-title">
  <h2 id="queue-title">Queue and workers</h2>
  {#if stale && stale > 0}<p class="warn" role="status"><span aria-hidden="true">!</span> <strong>{stale} stale attempt{stale === 1 ? '' : 's'}</strong> — workers stopped sending heartbeats.</p>{/if}
  <div class="columns">
    <div><h3>Tasks by status</h3>
      <dl>{#each taskRows as [status, count] (status)}<div class:zero={count === 0}><dt>{humanize(status.toLowerCase())}</dt><dd>{formatNumber(count)}</dd></div>{/each}</dl></div>
    <div><h3>Attempts by status</h3>
      <dl>{#each attemptRows as [status, count] (status)}<div class:zero={count === 0}><dt>{humanize(status)}</dt><dd>{formatNumber(count)}</dd></div>{/each}</dl></div>
  </div>
</section>

<style>
  section { display: grid; gap: .75rem; padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  h2, h3, p { margin: 0; } .warn { padding: .6rem .8rem; border-left: 4px solid #d8c98a; background: #292612; }
  .columns { display: grid; grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr)); gap: 1.25rem; }
  dl { display: grid; gap: .25rem; margin: .5rem 0 0; } dl div { display: flex; justify-content: space-between; gap: 1rem; padding: .2rem 0; border-bottom: 1px solid #262c26; }
  dt { color: #aeb5ad; } dd { margin: 0; font-weight: 800; font-variant-numeric: tabular-nums; } .zero dd { color: #929b92; font-weight: 400; }
</style>
