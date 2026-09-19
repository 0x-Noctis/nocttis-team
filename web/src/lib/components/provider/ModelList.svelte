<script lang="ts">
  import type { ModelResponse } from '$lib/api/types';
  import ModelCard from './ModelCard.svelte';

  let { models }: { models: ModelResponse[] } = $props();
</script>

<section aria-labelledby="provider-models-title">
  <h2 id="provider-models-title">Models <span>{models.length}</span></h2>
  {#if models.length === 0}
    <p class="empty">No models configured for this provider.</p>
  {:else}
    <div class="models">{#each models as model (model.id)}<ModelCard {model} />{/each}</div>
  {/if}
</section>

<style>
  section { display: grid; gap: 1rem; }
  h2 { margin: 0; color: #eef2ec; font-size: 1.25rem; } h2 span { color: #90e0a8; }
  .models { display: grid; gap: 1rem; }
  .empty { margin: 0; padding: 1.25rem; border: 1px dashed #525b52; color: #aeb5ad; }
</style>
