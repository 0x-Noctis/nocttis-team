<script lang="ts">
  import type { ApiError } from '$lib/api/types';
  let { error, onretry }: { error: ApiError; onretry?: () => void } = $props();
</script>

<section role="alert" aria-labelledby="provider-error-title">
  <div>
    <p class="eyebrow">Request failed</p>
    <h2 id="provider-error-title">{error.error.message}</h2>
    <p><code>{error.error.code}</code> · Request <code>{error.error.request_id}</code></p>
  </div>
  {#if onretry}<button type="button" onclick={onretry}>Try again</button>{/if}
</section>

<style>
  section { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 1rem; padding: 1.25rem; border-left: 4px solid #ff8b8b; background: #291313; color: #fff1f1; }
  .eyebrow { margin: 0 0 .35rem; color: #ffb0b0; font: 700 .75rem/1.2 ui-monospace, monospace; letter-spacing: .1em; text-transform: uppercase; }
  h2, p { margin: 0; } h2 { margin-bottom: .5rem; font-size: 1.15rem; }
  button { min-height: 2.75rem; padding: .65rem 1rem; border: 1px solid currentColor; background: transparent; color: inherit; font-weight: 800; cursor: pointer; }
  button:focus-visible { outline: 3px solid #fff; outline-offset: 3px; }
</style>
