<script lang="ts">
  import type { ProbeResult as Probe } from '$lib/api/types';

  let { probe }: { probe: Probe } = $props();
</script>

<article class:failed={probe.status === 'failed'} aria-label={`${probe.kind} probe ${probe.status}`}>
  <div>
    <strong>{probe.kind.replace('_', ' ')}</strong>
    <span class="status">{probe.status === 'succeeded' ? 'Succeeded' : 'Failed'}</span>
  </div>
  <dl>
    <div><dt>Verified</dt><dd>{probe.verified}</dd></div>
    <div><dt>Latency</dt><dd>{probe.latency_ms} ms</dd></div>
    {#if probe.error_code}<div><dt>Error code</dt><dd><code>{probe.error_code}</code></dd></div>{/if}
  </dl>
</article>

<style>
  article { padding: 1rem; border-left: 4px solid #90e0a8; background: #102217; }
  article.failed { border-color: #ff8b8b; background: #291313; }
  article > div, dl { display: flex; flex-wrap: wrap; justify-content: space-between; gap: .75rem; }
  strong { text-transform: capitalize; }
  .status { font-weight: 800; }
  dl { margin: 1rem 0 0; }
  dl div { display: flex; gap: .35rem; }
  dt { color: #aeb5ad; } dd { margin: 0; }
  code { color: inherit; }
</style>
