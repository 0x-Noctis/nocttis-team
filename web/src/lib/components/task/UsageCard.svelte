<script lang="ts">
  import { isNonNegativeSafeInteger, type TaskUsage } from '$lib/api/types';
  let { usage, loading = false, error = '' }: { usage?: TaskUsage; loading?: boolean; error?: string } = $props();
  const valid = (value: number) => isNonNegativeSafeInteger(value);
  const format = (value: number) => valid(value) ? value.toLocaleString() : 'Invalid';
  const usageValid = (value: TaskUsage) => [value.input_tokens, value.cached_tokens, value.output_tokens, value.total_tokens].every(valid);
</script>

<section aria-labelledby="usage-card-title"><header><h2 id="usage-card-title">Model usage</h2>{#if usage}<strong>{usage.estimated ? 'Estimated' : 'Measured'}</strong>{/if}</header>
  {#if loading}<p aria-live="polite" aria-busy="true">Loading usage…</p>
  {:else if error}<p class="error" role="alert">Usage unavailable: {error}</p>
  {:else if !usage}<p>No usage recorded.</p>
  {:else if !usageValid(usage)}<p class="error" role="alert">Usage contains an unsafe integer.</p>
  {:else}<dl><div><dt>Input</dt><dd>{format(usage.input_tokens)}</dd></div><div><dt>Cached</dt><dd>{format(usage.cached_tokens)}</dd></div><div><dt>Output</dt><dd>{format(usage.output_tokens)}</dd></div><div><dt>Total</dt><dd>{format(usage.total_tokens)}</dd></div></dl>{/if}
</section>

<style>
  section { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; } header { display: flex; justify-content: space-between; gap: 1rem; } h2 { margin-top: 0; } header strong { color: #90e0a8; }
  dl { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: .75rem; margin-bottom: 0; } dl div { padding: .75rem; border: 1px solid #394139; } dt { color: #aeb5ad; } dd { margin: .3rem 0 0; font-size: 1.2rem; font-weight: 800; } .error { color: #ffb0b0; }
  @media (max-width: 620px) { dl { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
</style>
