<script lang="ts">
  import type { LeaseConflict, SlotLease } from './types';

  let { leases, conflicts = [], loading = false, error = '' }: { leases: SlotLease[]; conflicts?: LeaseConflict[]; loading?: boolean; error?: string } = $props();
</script>

<section aria-labelledby="leases-title">
  <h2 id="leases-title">File leases</h2>
  {#if loading}<p aria-live="polite" aria-busy="true">Loading leases…</p>
  {:else if error}<p class="bad" role="alert">Leases unavailable: {error}</p>
  {:else}
    {#if conflicts.length}
      <div class="conflicts" role="alert"><h3>Held back by a lease conflict</h3>
        <ul>{#each conflicts as conflict (conflict.task_id + conflict.pattern)}<li><strong>{conflict.task_id}</strong> needs <code>{conflict.pattern}</code>, held by <strong>{conflict.holder_task_id}</strong>.</li>{/each}</ul></div>
    {/if}
    {#if leases.length}
      <table><caption class="sr">Active file leases</caption><thead><tr><th scope="col">Scope</th><th scope="col">Expires in</th></tr></thead>
        <tbody>{#each leases as lease (lease.pattern)}<tr><td><code>{lease.pattern}</code></td><td>{lease.expires_in_seconds}s</td></tr>{/each}</tbody></table>
    {:else}<p>No active leases.</p>{/if}
  {/if}
</section>

<style>
  section { display: grid; gap: .75rem; padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; } h2, h3 { margin: 0; } h3 { font-size: 1rem; }
  .conflicts { padding: .75rem; border: 1px solid #ff8b8b; color: #ffb0b0; } .conflicts ul { margin: .4rem 0 0; padding-left: 1.2rem; }
  table { width: 100%; border-collapse: collapse; } th, td { padding: .4rem .5rem; border-bottom: 1px solid #394139; text-align: left; } th { color: #aeb5ad; font-size: .8rem; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); } .bad { color: #ffb0b0; }
</style>
