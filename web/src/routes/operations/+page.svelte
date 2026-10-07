<script lang="ts">
  import { onMount } from 'svelte';
  import { asApiError, operationsApi } from '$lib/api/client';
  import type { ApiError } from '$lib/api/types';
  import { startPoller } from '$lib/realtime/run-poller';
  import { HealthPanel, ProviderReadiness, QueueSummary, RetentionPanel, UsagePanel, type OperationsView } from '$lib/components/metrics';

  const POLL_MS = 10_000;

  let view = $state<OperationsView | null>(null);
  let loading = $state(true);
  let error = $state<ApiError | null>(null);
  let liveError = $state('');
  let updatedAt = $state('');

  async function refresh(): Promise<undefined> {
    view = await operationsApi.summary();
    updatedAt = new Date().toISOString().slice(11, 19) + ' UTC';
    error = null;
  }

  onMount(() => {
    let poller: ReturnType<typeof startPoller> | undefined;
    let disposed = false;
    const onVisible = () => { if (!document.hidden) poller?.refresh(); };
    refresh()
      .then(() => {
        if (disposed) return;
        poller = startPoller(refresh, {
          intervalMs: POLL_MS,
          isVisible: () => !document.hidden,
          onError: (_reason, failures) => { liveError = `Live updates interrupted (attempt ${failures}). Retrying automatically.`; },
          onRecover: () => { liveError = ''; }
        });
        document.addEventListener('visibilitychange', onVisible);
      })
      .catch((reason) => { if (!disposed) error = asApiError(reason); })
      .finally(() => { if (!disposed) loading = false; });
    return () => {
      disposed = true;
      poller?.stop();
      document.removeEventListener('visibilitychange', onVisible);
    };
  });
</script>

<svelte:head><title>Operations · Noctis</title></svelte:head>

<main>
  <nav aria-label="Breadcrumb"><a href="/">Home</a><span aria-hidden="true">/</span><span>Operations</span></nav>
  <header>
    <div><p class="eyebrow">Control plane</p><h1>Operations</h1><p>Health, queue, usage, provider readiness, and cleanup status. Read-only.</p></div>
    <p class="updated" aria-live="off">{#if updatedAt}Updated {updatedAt}{/if} · <a href="/settings">Settings</a></p>
  </header>

  {#if error}<section class="error" role="alert"><strong>{error.error.message}</strong><span>Request ID: <code>{error.error.request_id}</code></span></section>{/if}
  {#if liveError}<p class="warning" role="status">{liveError}</p>{/if}

  {#if loading}<p class="state" aria-live="polite" aria-busy="true">Loading operations…</p>
  {:else if view}
    <HealthPanel {view} />
    {#if view.tasks && view.attempts}<QueueSummary tasks={view.tasks} attempts={view.attempts} stale={view.stale_attempts} />{/if}
    {#if view.usage}<UsagePanel usage={view.usage} />{/if}
    {#if view.providers}<ProviderReadiness providers={view.providers} />{/if}
    {#if view.retention}<RetentionPanel retention={view.retention} />{/if}
    {#if !view.tasks}<p class="warning" role="status"><span aria-hidden="true">!</span> Queue, usage, provider, and retention data are unavailable while the database is down.</p>{/if}
  {/if}
</main>

<style>
  :global(*) { box-sizing: border-box; }
  :global(body) { margin: 0; background: #090b09; color: #eef2ec; font-family: system-ui, sans-serif; }
  main { display: grid; gap: 1rem; max-width: 76rem; margin: auto; padding: 2rem; }
  nav { display: flex; gap: .6rem; color: #aeb5ad; } a { color: #90e0a8; }
  header { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 1rem; align-items: end; padding: 1.5rem 0 .5rem; border-bottom: 1px solid #394139; }
  h1 { margin: 0; font-size: clamp(2rem, 6vw, 3.5rem); } .eyebrow { margin: 0; color: #90e0a8; font: 700 .8rem monospace; text-transform: uppercase; } header p { margin: .3rem 0 0; color: #aeb5ad; }
  .updated { font-variant-numeric: tabular-nums; }
  .error, .warning, .state { padding: 1rem; border: 1px solid #697169; margin: 0; } .error { display: grid; gap: .4rem; border-color: #ff8b8b; color: #ffb0b0; } .warning { border-color: #d8c98a; color: #f2e6aa; }
  @media (max-width: 700px) { main { padding: 1rem; } }
</style>
