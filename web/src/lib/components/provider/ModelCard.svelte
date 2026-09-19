<script lang="ts">
  import { isPositiveSafeInteger, type ModelResponse } from '$lib/api/types';
  import CapabilityMatrix from './CapabilityMatrix.svelte';

  let { model }: { model: ModelResponse } = $props();
</script>

<article aria-labelledby={`model-${model.id}`}>
  <header>
    <div>
      <p>{model.class}</p>
      <h3 id={`model-${model.id}`}>{model.id}</h3>
      <code>{model.remote_name}</code>
    </div>
    <dl>
      <div><dt>Context</dt><dd>{isPositiveSafeInteger(model.context_window) ? String(model.context_window) : 'Invalid value'}</dd></div>
      <div><dt>Max output</dt><dd>{isPositiveSafeInteger(model.max_output_tokens) ? String(model.max_output_tokens) : 'Invalid value'}</dd></div>
    </dl>
  </header>
  <CapabilityMatrix capabilities={model.capabilities} />
</article>

<style>
  article { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  header { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 1rem; margin-bottom: 1rem; }
  p { margin: 0 0 .3rem; color: #90e0a8; font: 700 .75rem/1.2 ui-monospace, monospace; letter-spacing: .1em; text-transform: uppercase; }
  h3 { margin: 0 0 .35rem; font-size: 1.25rem; }
  code { color: #b9c2b9; }
  dl { display: flex; flex-wrap: wrap; gap: 1rem; margin: 0; }
  dl div { min-width: 6rem; } dt { color: #aeb5ad; font-size: .75rem; } dd { margin: .2rem 0 0; font-weight: 800; }
</style>
