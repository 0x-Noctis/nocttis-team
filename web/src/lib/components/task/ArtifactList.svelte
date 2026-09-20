<script lang="ts">
  import { isNonNegativeSafeInteger, type TaskArtifact } from '$lib/api/types';
  let { artifacts, loading = false, error = '' }: { artifacts: TaskArtifact[]; loading?: boolean; error?: string } = $props();
  const size = (bytes: number) => isNonNegativeSafeInteger(bytes) ? `${bytes.toLocaleString()} bytes` : 'Invalid size';
</script>

<section aria-labelledby="artifact-list-title"><h2 id="artifact-list-title">Artifacts</h2>
  {#if loading}<p aria-live="polite" aria-busy="true">Loading artifacts…</p>
  {:else if error}<p class="error" role="alert">Artifacts unavailable: {error}</p>
  {:else if artifacts.length === 0}<p>No artifacts submitted.</p>
  {:else}<ul>{#each artifacts as artifact (artifact.id)}<li><div><strong>{artifact.name}</strong><span>{artifact.kind.replace('_', ' ')} · {size(artifact.size_bytes)}</span></div><a href={artifact.download_url}>Open artifact <span class="sr-only">{artifact.name}</span></a></li>{/each}</ul>{/if}
</section>

<style>
  section { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; } h2 { margin-top: 0; }
  ul { display: grid; gap: .75rem; margin: 0; padding: 0; list-style: none; } li { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 1rem; padding: .8rem; border: 1px solid #394139; }
  strong, span { display: block; } li span { margin-top: .25rem; color: #aeb5ad; } a { color: #b7dcff; } a:focus-visible { outline: 3px solid #fff; outline-offset: 3px; } .error { color: #ffb0b0; }
  .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border: 0; }
</style>
