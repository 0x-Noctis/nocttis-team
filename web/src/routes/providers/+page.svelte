<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiRequestError, providerApi } from '$lib/api/client';
  import {
    ErrorPanel,
    ModelCard,
    ProbeResult,
    ProviderForm
  } from '$lib/components/provider';
  import {
    MAX_SAFE_INTEGER,
    capabilityKeys,
    isPositiveSafeInteger,
    type ApiError,
    type ClaimedCapabilities,
    type ModelInput,
    type ModelResponse,
    type ProbeKind,
    type ProbeResult as Probe,
    type ProviderInput,
    type ProviderResponse
  } from '$lib/api/types';

  const emptyCapabilities = (): ClaimedCapabilities => ({
    chat: false,
    streaming: false,
    tools: false,
    parallel_tools: false
  });

  const emptyModel = (providerId = ''): ModelInput => ({
    id: '',
    provider_id: providerId,
    remote_name: '',
    class: '',
    context_window: 1,
    max_output_tokens: 1,
    claimed_capabilities: emptyCapabilities()
  });

  let providers = $state<ProviderResponse[]>([]);
  let selectedProviderId = $state('');
  let models = $state<ModelResponse[]>([]);
  let probes = $state<Probe[]>([]);
  let loading = $state(true);
  let loadingModels = $state(false);
  let submitting = $state(false);
  let error = $state<ApiError | null>(null);
  let notice = $state('');
  let providerEditor = $state<'closed' | 'create' | 'edit'>('closed');
  let modelEditor = $state<'closed' | 'create' | 'edit'>('closed');
  let editingModelId = $state('');
  let modelDraft = $state<ModelInput>(emptyModel());
  let modelNumericError = $state('');
  let probing = $state('');

  const selectedProvider = $derived(providers.find(({ id }) => id === selectedProviderId));

  function apiError(reason: unknown): ApiError {
    if (reason instanceof ApiRequestError) return reason.envelope;
    return {
      error: {
        code: 'NETWORK_ERROR',
        message: reason instanceof Error ? reason.message : 'Request gagal.',
        details: {},
        request_id: 'tidak tersedia'
      }
    };
  }

  async function loadProviders() {
    loading = true;
    error = null;
    try {
      providers = await providerApi.list();
      if (!providers.some(({ id }) => id === selectedProviderId)) {
        selectedProviderId = providers[0]?.id ?? '';
      }
      if (selectedProviderId) await loadModels(selectedProviderId);
      else models = [];
    } catch (reason) {
      error = apiError(reason);
    } finally {
      loading = false;
    }
  }

  async function loadModels(providerId: string) {
    loadingModels = true;
    error = null;
    try {
      models = await providerApi.models(providerId);
      probes = [];
    } catch (reason) {
      error = apiError(reason);
    } finally {
      loadingModels = false;
    }
  }

  async function selectProvider(providerId: string) {
    selectedProviderId = providerId;
    providerEditor = 'closed';
    modelEditor = 'closed';
    notice = '';
    await loadModels(providerId);
  }

  async function saveProvider(provider: ProviderInput) {
    submitting = true;
    error = null;
    try {
      const saved = providerEditor === 'edit' && selectedProvider
        ? await providerApi.update(selectedProvider.id, provider)
        : await providerApi.create(provider);
      notice = `Provider ${saved.id} tersimpan.`;
      providerEditor = 'closed';
      await loadProviders();
      await selectProvider(saved.id);
    } catch (reason) {
      error = apiError(reason);
    } finally {
      submitting = false;
    }
  }

  async function deleteProvider() {
    if (!selectedProvider || !confirm(`Hapus provider ${selectedProvider.id} dan semua modelnya?`)) return;
    submitting = true;
    error = null;
    try {
      await providerApi.delete(selectedProvider.id);
      notice = `Provider ${selectedProvider.id} dihapus.`;
      selectedProviderId = '';
      await loadProviders();
    } catch (reason) {
      error = apiError(reason);
    } finally {
      submitting = false;
    }
  }

  function editModel(model?: ModelResponse) {
    modelEditor = model ? 'edit' : 'create';
    editingModelId = model?.id ?? '';
    modelDraft = model
      ? {
          id: model.id,
          provider_id: model.provider_id,
          remote_name: model.remote_name,
          class: model.class,
          context_window: model.context_window,
          max_output_tokens: model.max_output_tokens,
          claimed_capabilities: { ...model.capabilities.claimed }
        }
      : emptyModel(selectedProviderId);
    modelNumericError = '';
  }

  async function saveModel(event: SubmitEvent) {
    event.preventDefault();
    if (!isPositiveSafeInteger(modelDraft.context_window) || !isPositiveSafeInteger(modelDraft.max_output_tokens)) {
      modelNumericError = `Nilai numeric harus bilangan bulat dari 1 sampai ${MAX_SAFE_INTEGER}.`;
      return;
    }
    submitting = true;
    error = null;
    modelNumericError = '';
    try {
      if (modelEditor === 'edit') await providerApi.updateModel(editingModelId, modelDraft);
      else await providerApi.createModel(selectedProviderId, modelDraft);
      notice = `Model ${modelDraft.id} tersimpan.`;
      modelEditor = 'closed';
      await loadModels(selectedProviderId);
    } catch (reason) {
      error = apiError(reason);
    } finally {
      submitting = false;
    }
  }

  async function deleteModel(model: ModelResponse) {
    if (!confirm(`Hapus model ${model.id}?`)) return;
    submitting = true;
    error = null;
    try {
      await providerApi.deleteModel(model.id);
      notice = `Model ${model.id} dihapus.`;
      await loadModels(selectedProviderId);
    } catch (reason) {
      error = apiError(reason);
    } finally {
      submitting = false;
    }
  }

  async function runProbe(model: ModelResponse, kind: ProbeKind) {
    probing = `${model.id}:${kind}`;
    error = null;
    try {
      const { result } = await providerApi.probe(model.id, kind);
      probes = [...probes.filter((probe) => probe.model_id !== model.id || probe.kind !== kind), result];
      notice = `Probe ${kind} untuk ${model.id} selesai.`;
      models = await providerApi.models(selectedProviderId);
    } catch (reason) {
      error = apiError(reason);
    } finally {
      probing = '';
    }
  }

  onMount(loadProviders);
</script>

<svelte:head><title>Providers · Noctis</title></svelte:head>

<main>
  <header class="page-header">
    <div><p class="eyebrow">NOCTIS / PROVIDERS</p><h1>Providers</h1><p>Kelola endpoint OpenAI-compatible, model, dan capability probe.</p></div>
    <a href="/">Kembali ke beranda</a>
  </header>

  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  {#if error}<ErrorPanel {error} onretry={loadProviders} />{/if}

  {#if loading}
    <section class="state" aria-busy="true" aria-live="polite"><h2>Memuat providers…</h2></section>
  {:else}
    <div class="toolbar">
      <h2>Daftar provider <span>{providers.length}</span></h2>
      <button type="button" onclick={() => providerEditor = 'create'}>Tambah provider</button>
    </div>

    {#if providers.length === 0}
      <section class="state"><h2>Belum ada provider</h2><p>Tambahkan endpoint tanpa memasukkan nilai API key.</p></section>
    {:else}
      <nav aria-label="Provider">
        {#each providers as provider (provider.id)}
          <button class:active={provider.id === selectedProviderId} type="button" onclick={() => selectProvider(provider.id)}>
            <strong>{provider.id}</strong><span>Secret {provider.secret_configured ? 'configured' : 'missing'}</span>
          </button>
        {/each}
      </nav>
    {/if}

    {#if providerEditor !== 'closed'}
      <section class="editor" aria-label={providerEditor === 'edit' ? 'Edit provider' : 'Create provider'}>
        <ProviderForm
          initial={providerEditor === 'edit' && selectedProvider ? {
            id: selectedProvider.id,
            base_url: selectedProvider.base_url,
            api_key_env: selectedProvider.api_key_env,
            request_timeout_seconds: selectedProvider.request_timeout_seconds
          } : undefined}
          {submitting}
          onsubmit={saveProvider}
        />
        <button class="secondary" type="button" onclick={() => providerEditor = 'closed'}>Batal</button>
      </section>
    {/if}

    {#if selectedProvider}
      <section class="provider-detail" aria-labelledby="selected-provider-title">
        <header>
          <div><p class="eyebrow">PROVIDER</p><h2 id="selected-provider-title">{selectedProvider.id}</h2><code>{selectedProvider.base_url}</code></div>
          <div class="actions"><button type="button" onclick={() => providerEditor = 'edit'}>Edit provider</button><button class="danger" type="button" disabled={submitting} onclick={deleteProvider}>Hapus provider</button></div>
        </header>
        <dl class="provider-meta">
          <div><dt>Secret</dt><dd>{selectedProvider.secret_configured ? 'configured' : 'missing'}</dd></div>
          <div><dt>Environment</dt><dd><code>{selectedProvider.api_key_env}</code></dd></div>
          <div><dt>Timeout</dt><dd>{String(selectedProvider.request_timeout_seconds)} detik</dd></div>
        </dl>

        <div class="toolbar"><h2>Models <span>{models.length}</span></h2><button type="button" onclick={() => editModel()}>Tambah model</button></div>
        {#if modelEditor !== 'closed'}
          <form class="model-form" onsubmit={saveModel} aria-labelledby="model-form-title">
            <h3 id="model-form-title">{modelEditor === 'edit' ? 'Edit model' : 'Tambah model'}</h3>
            <div class="fields">
              <label>Model ID<input bind:value={modelDraft.id} required autocomplete="off" /></label>
              <label>Remote model name<input bind:value={modelDraft.remote_name} required autocomplete="off" /></label>
              <label>Class<input bind:value={modelDraft.class} required autocomplete="off" /></label>
              <label>Context window<input bind:value={modelDraft.context_window} type="number" min="1" max={MAX_SAFE_INTEGER} step="1" required /></label>
              <label>Max output tokens<input bind:value={modelDraft.max_output_tokens} type="number" min="1" max={MAX_SAFE_INTEGER} step="1" required /></label>
            </div>
            <fieldset><legend>Claimed capabilities</legend>{#each capabilityKeys as capability}<label><input type="checkbox" bind:checked={modelDraft.claimed_capabilities[capability]} /> {capability.replace('_', ' ')}</label>{/each}</fieldset>
            {#if modelNumericError}<p class="validation" role="alert">{modelNumericError}</p>{/if}
            <div class="actions"><button type="submit" disabled={submitting}>Simpan model</button><button class="secondary" type="button" onclick={() => modelEditor = 'closed'}>Batal</button></div>
          </form>
        {/if}

        {#if loadingModels}
          <section class="state" aria-busy="true"><p>Memuat model…</p></section>
        {:else if models.length === 0}
          <section class="state"><h3>Belum ada model</h3><p>Tambahkan model untuk menjalankan probe.</p></section>
        {:else}
          <div class="models">
            {#each models as model (model.id)}
              <section class="model-entry">
                <ModelCard {model} />
                <div class="actions model-actions">
                  {#each ['chat', 'streaming', 'tools'] as kind}
                    <button type="button" disabled={probing !== ''} onclick={() => runProbe(model, kind as ProbeKind)}>{probing === `${model.id}:${kind}` ? 'Menjalankan…' : `Probe ${kind}`}</button>
                  {/each}
                  <button class="secondary" type="button" onclick={() => editModel(model)}>Edit</button>
                  <button class="danger" type="button" disabled={submitting} onclick={() => deleteModel(model)}>Hapus</button>
                </div>
                {#each probes.filter((probe) => probe.model_id === model.id) as probe (`${probe.model_id}:${probe.kind}`)}<ProbeResult {probe} />{/each}
              </section>
            {/each}
          </div>
        {/if}
      </section>
    {/if}
  {/if}
</main>

<style>
  :global(*) { box-sizing: border-box; }
  :global(body) { margin: 0; background: #0b0d0c; color: #eef2ec; font-family: Inter, ui-sans-serif, system-ui, sans-serif; }
  main { width: min(1120px, calc(100% - 2rem)); margin: auto; padding: 3rem 0 6rem; }
  .page-header, .provider-detail > header, .toolbar, .actions { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 1rem; }
  .page-header { align-items: end; margin-bottom: 2rem; } h1 { margin: .25rem 0; font-size: clamp(3rem, 9vw, 7rem); line-height: .9; letter-spacing: -.06em; }
  h2, h3, p { margin-top: 0; } .eyebrow { margin-bottom: .35rem; color: #90e0a8; font: 700 .75rem ui-monospace, monospace; letter-spacing: .12em; }
  a { color: #90e0a8; } nav { display: flex; gap: .5rem; overflow-x: auto; margin: 1rem 0; padding-bottom: .5rem; }
  nav button { min-width: 12rem; display: grid; gap: .25rem; text-align: left; background: #121512; color: #eef2ec; border: 1px solid #394139; }
  nav button.active { border-color: #90e0a8; } nav span, dt { color: #aeb5ad; font-size: .8rem; }
  button { min-height: 2.75rem; padding: .65rem 1rem; border: 0; background: #90e0a8; color: #09100b; font-weight: 800; cursor: pointer; }
  button.secondary { border: 1px solid #748074; background: transparent; color: #eef2ec; } button.danger { background: #ffb0b0; color: #260707; }
  button:disabled { opacity: .55; cursor: wait; } button:focus-visible, a:focus-visible, input:focus-visible { outline: 3px solid #fff; outline-offset: 3px; }
  .toolbar { margin: 1.5rem 0 .75rem; } .toolbar h2 { margin: 0; } .toolbar span { color: #90e0a8; }
  .state, .editor, .provider-detail, .model-form { padding: 1.25rem; border: 1px solid #394139; background: #121512; }
  .editor { display: grid; gap: 1rem; margin: 1rem 0; } .editor > button { justify-self: start; }
  .provider-detail { display: grid; gap: 1.25rem; margin-top: 1rem; } code { overflow-wrap: anywhere; color: #c6d8c9; }
  .provider-meta { display: flex; flex-wrap: wrap; gap: 1.5rem; margin: 0; } .provider-meta div { display: grid; gap: .25rem; } dd { margin: 0; }
  .model-form { display: grid; gap: 1rem; background: #090b0a; } .fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; }
  label { display: grid; gap: .4rem; font-weight: 700; } input { min-height: 2.75rem; padding: .6rem; border: 1px solid #525b52; background: #121512; color: inherit; font: inherit; }
  fieldset { display: flex; flex-wrap: wrap; gap: 1rem; border: 1px solid #525b52; } fieldset label { display: flex; align-items: center; } fieldset input { min-height: auto; }
  .models, .model-entry { display: grid; gap: 1rem; } .model-entry { padding: 1rem; border: 1px solid #394139; } .model-actions { justify-content: flex-start; }
  .notice { padding: 1rem; border-left: 4px solid #90e0a8; background: #102217; } .validation { color: #ffb0b0; }
  @media (max-width: 680px) { .fields { grid-template-columns: 1fr; } .actions button { flex: 1 1 auto; } }
  @media (prefers-reduced-motion: reduce) { :global(*) { scroll-behavior: auto !important; } }
</style>
