<script lang="ts">
  import { estimatedPercent, formatCost, formatNumber, totalTokens } from './helpers';
  import type { UsageView } from './types';

  let { usage }: { usage: UsageView } = $props();
  const percent = $derived(estimatedPercent(usage));
</script>

<section aria-labelledby="usage-title">
  <h2 id="usage-title">Usage and cost</h2>
  <dl>
    <div><dt>Input tokens</dt><dd>{formatNumber(usage.input_tokens)}</dd></div>
    <div><dt>Output tokens</dt><dd>{formatNumber(usage.output_tokens)}</dd></div>
    <div><dt>Total tokens</dt><dd>{formatNumber(totalTokens(usage))}</dd></div>
    <div><dt>Cost</dt><dd class="cost">{formatCost(usage.cost)}</dd></div>
  </dl>
  {#if usage.estimated_tokens > 0}
    <p class="estimated" role="note"><span aria-hidden="true">≈</span> <strong>Estimated:</strong> {formatNumber(usage.estimated_tokens)} of these tokens ({percent}%) are estimates made locally because the provider did not report usage. Treat totals as approximate.</p>
  {:else}
    <p class="exact">All token counts were reported by providers.</p>
  {/if}
</section>

<style>
  section { display: grid; gap: .75rem; padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  h2, p { margin: 0; } dl { display: grid; grid-template-columns: repeat(auto-fit, minmax(12rem, 1fr)); gap: .75rem; margin: 0; }
  dl div { display: grid; gap: .2rem; padding: .7rem; border: 1px solid #394139; } dt { color: #aeb5ad; } dd { margin: 0; font-size: 1.3rem; font-weight: 800; font-variant-numeric: tabular-nums; overflow-wrap: anywhere; } dd.cost { font-size: 1rem; }
  .estimated { padding: .6rem .8rem; border-left: 4px solid #d8c98a; background: #292612; line-height: 1.5; } .exact { color: #aeb5ad; }
</style>
