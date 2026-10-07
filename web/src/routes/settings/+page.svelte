<script lang="ts">
  import { onMount } from 'svelte';
  import { asApiError, operationsApi } from '$lib/api/client';
  import type { ApiError } from '$lib/api/types';
  import { ConfigTable, type ConfigView } from '$lib/components/metrics';

  let config = $state<ConfigView | null>(null);
  let loading = $state(true);
  let error = $state<ApiError | null>(null);

  onMount(() => {
    operationsApi.config()
      .then((value) => { config = value; })
      .catch((reason) => { error = asApiError(reason); })
      .finally(() => { loading = false; });
  });
</script>

<svelte:head><title>Settings · Noctis</title></svelte:head>

<main>
  <nav aria-label="Breadcrumb"><a href="/">Home</a><span aria-hidden="true">/</span><span>Settings</span></nav>
  <header>
    <div><p class="eyebrow">Control plane</p><h1>Settings</h1>
      <p>Effective non-secret configuration of this server. Read-only: change values in the config file or with <code>NOCTIS__SECTION__FIELD</code> environment variables, then restart.</p></div>
  </header>
  <p class="note" role="note"><span aria-hidden="true">ℹ</span> Secrets (API keys, database credentials) and server file paths are never shown here. Provider keys live only in server environment variables.</p>

  {#if error}<section class="error" role="alert"><strong>{error.error.message}</strong><span>Request ID: <code>{error.error.request_id}</code></span></section>{/if}
  {#if loading}<p class="state" aria-live="polite" aria-busy="true">Loading settings…</p>
  {:else if config}<ConfigTable {config} />{/if}
</main>

<style>
  :global(*) { box-sizing: border-box; }
  :global(body) { margin: 0; background: #090b09; color: #eef2ec; font-family: system-ui, sans-serif; }
  main { display: grid; gap: 1rem; max-width: 76rem; margin: auto; padding: 2rem; }
  nav { display: flex; gap: .6rem; color: #aeb5ad; } a { color: #90e0a8; }
  header { padding: 1.5rem 0 .5rem; border-bottom: 1px solid #394139; } h1 { margin: 0; font-size: clamp(2rem, 6vw, 3.5rem); }
  .eyebrow { margin: 0; color: #90e0a8; font: 700 .8rem monospace; text-transform: uppercase; } header p { margin: .3rem 0 0; color: #aeb5ad; max-width: 52rem; line-height: 1.5; } code { color: #90e0a8; }
  .note { margin: 0; padding: .75rem 1rem; border-left: 4px solid #7aa7d8; background: #101d29; }
  .error, .state { padding: 1rem; border: 1px solid #697169; margin: 0; } .error { display: grid; gap: .4rem; border-color: #ff8b8b; color: #ffb0b0; }
  @media (max-width: 700px) { main { padding: 1rem; } }
</style>
