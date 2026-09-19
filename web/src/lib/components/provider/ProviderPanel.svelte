<script lang="ts">
  import { isPositiveSafeInteger, type ProviderViewState } from '$lib/api/types';
  import ErrorPanel from './ErrorPanel.svelte';
  import ModelList from './ModelList.svelte';
  import ProbeResult from './ProbeResult.svelte';

  let { state, onretry }: { state: ProviderViewState; onretry?: () => void } = $props();
</script>

{#if state.status === 'loading'}
  <section class="state" aria-live="polite" aria-busy="true">
    <span class="spinner" aria-hidden="true"></span>
    <div><h2>Loading providers</h2><p>Reading provider and capability status…</p></div>
  </section>
{:else if state.status === 'empty'}
  <section class="state"><div><h2>No providers configured</h2><p>Add an OpenAI-compatible endpoint to begin capability probes.</p></div></section>
{:else if state.status === 'error'}
  <ErrorPanel error={state.error} {onretry} />
{:else}
  <section class="provider" aria-labelledby={`provider-${state.provider.id}`}>
    <header>
      <div>
        <p class="eyebrow">Provider</p>
        <h2 id={`provider-${state.provider.id}`}>{state.provider.id}</h2>
        <code>{state.provider.base_url}</code>
      </div>
      <div class:missing={!state.provider.secret_configured} class="secret-status">
        <strong>Secret {state.provider.secret_configured ? 'configured' : 'missing'}</strong>
        <span>{state.provider.api_key_env}</span>
      </div>
    </header>
    <p class="timeout">Request timeout: {isPositiveSafeInteger(state.provider.request_timeout_seconds) ? `${String(state.provider.request_timeout_seconds)} seconds` : 'Invalid value'}</p>
    <ModelList models={state.models} />
    <section class="probes" aria-labelledby="probe-results-title">
      <h3 id="probe-results-title">Probe results</h3>
      {#if state.probes.length === 0}
        <p class="empty">No probe results yet.</p>
      {:else}
        <div>{#each state.probes as probe (`${probe.model_id}-${probe.kind}`)}<ProbeResult {probe} />{/each}</div>
      {/if}
    </section>
  </section>
{/if}

<style>
  .state, .provider { padding: 1.5rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  .state { display: flex; align-items: center; gap: 1rem; min-height: 8rem; }
  .state h2, .state p { margin: 0; } .state p { margin-top: .35rem; color: #aeb5ad; }
  .spinner { width: 1.5rem; height: 1.5rem; border: 3px solid #525b52; border-top-color: #90e0a8; border-radius: 50%; animation: spin .8s linear infinite; }
  .provider { display: grid; gap: 1.5rem; }
  header { display: flex; flex-wrap: wrap; align-items: start; justify-content: space-between; gap: 1rem; }
  .eyebrow { margin: 0 0 .35rem; color: #90e0a8; font: 700 .75rem/1.2 ui-monospace, monospace; letter-spacing: .1em; text-transform: uppercase; }
  h2, h3 { margin: 0; } header code { display: inline-block; margin-top: .4rem; color: #b9c2b9; }
  .secret-status { display: grid; gap: .2rem; padding: .65rem .8rem; border: 1px solid #55705c; background: #102217; }
  .secret-status.missing { border-color: #d8c98a; background: #292612; }
  .secret-status span { color: #c6cec6; font: .8rem ui-monospace, monospace; }
  .timeout { margin: -1rem 0 0; color: #aeb5ad; }
  .probes { display: grid; gap: 1rem; } .probes > div { display: grid; gap: .75rem; grid-template-columns: repeat(auto-fit, minmax(15rem, 1fr)); }
  .empty { margin: 0; padding: 1rem; border: 1px dashed #525b52; color: #aeb5ad; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { .spinner { animation: none; border-color: #90e0a8; } }
</style>
