<script lang="ts">
  import type { ProviderInput } from '$lib/api/types';

  let {
    initial = {
      id: '',
      base_url: '',
      api_key_env: '',
      request_timeout_seconds: 180
    },
    submitting = false,
    onsubmit
  }: {
    initial?: ProviderInput;
    submitting?: boolean;
    onsubmit?: (provider: ProviderInput) => void;
  } = $props();

  let provider = $state<ProviderInput>({
    id: '',
    base_url: '',
    api_key_env: '',
    request_timeout_seconds: 180
  });

  $effect(() => {
    provider = { ...initial };
  });

  function submit(event: SubmitEvent) {
    event.preventDefault();
    onsubmit?.({ ...provider });
  }
</script>

<form onsubmit={submit} aria-labelledby="provider-form-title">
  <div class="heading">
    <div>
      <p class="eyebrow">Provider configuration</p>
      <h2 id="provider-form-title">OpenAI-compatible provider</h2>
    </div>
    <p class="security-note">Secret value tetap di server.</p>
  </div>

  <div class="fields">
    <label>
      Provider ID
      <input bind:value={provider.id} name="id" autocomplete="off" required />
    </label>
    <label>
      Base URL
      <input bind:value={provider.base_url} name="base_url" type="url" placeholder="https://api.example.com/v1" required />
    </label>
    <label>
      API key environment variable
      <input bind:value={provider.api_key_env} name="api_key_env" pattern="[A-Za-z_][A-Za-z0-9_]*" placeholder="PRIMARY_API_KEY" autocomplete="off" required />
      <span>Masukkan nama environment variable, bukan API key.</span>
    </label>
    <label>
      Request timeout (seconds)
      <input bind:value={provider.request_timeout_seconds} name="request_timeout_seconds" type="number" min="1" step="1" required />
    </label>
  </div>

  <button type="submit" disabled={submitting}>{submitting ? 'Saving provider…' : 'Save provider'}</button>
</form>

<style>
  form { display: grid; gap: 1.5rem; padding: 1.5rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  .heading { display: flex; flex-wrap: wrap; align-items: start; justify-content: space-between; gap: 1rem; }
  .eyebrow { margin: 0 0 .35rem; color: #90e0a8; font: 700 .75rem/1.2 ui-monospace, monospace; letter-spacing: .12em; text-transform: uppercase; }
  h2 { margin: 0; font-size: 1.4rem; }
  .security-note { margin: 0; padding: .45rem .7rem; border: 1px solid #55705c; color: #c6d8c9; font-size: .875rem; }
  .fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; }
  label { display: grid; gap: .45rem; font-weight: 700; }
  label span { color: #aeb5ad; font-size: .8rem; font-weight: 400; }
  input { width: 100%; min-height: 2.75rem; padding: .65rem .75rem; border: 1px solid #525b52; background: #090b0a; color: inherit; font: inherit; }
  input:focus-visible, button:focus-visible { outline: 3px solid #fff; outline-offset: 3px; }
  button { justify-self: start; min-height: 2.75rem; padding: .65rem 1rem; border: 0; background: #90e0a8; color: #09100b; font-weight: 800; cursor: pointer; }
  button:disabled { opacity: .6; cursor: wait; }
  @media (max-width: 680px) { .fields { grid-template-columns: 1fr; } }
</style>
