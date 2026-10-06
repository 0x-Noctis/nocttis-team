<script lang="ts">
  import type { QueueItem, QueueReason } from './types';

  let { items, loading = false, error = '' }: { items: QueueItem[]; loading?: boolean; error?: string } = $props();

  const reasons: Record<QueueReason, { icon: string; label: string }> = {
    slot: { icon: '…', label: 'Waiting for a free slot' },
    dependency: { icon: '⛓', label: 'Waiting for a dependency' },
    lease: { icon: '🔒', label: 'Waiting for a file lease' },
    budget: { icon: '◔', label: 'Waiting for token budget' },
    paused: { icon: '‖', label: 'Run is paused' }
  };
</script>

<section aria-labelledby="queue-title">
  <h2 id="queue-title">Queue{#if items.length} <span>({items.length})</span>{/if}</h2>
  {#if loading}<p aria-live="polite" aria-busy="true">Loading queue…</p>
  {:else if error}<p class="bad" role="alert">Queue unavailable: {error}</p>
  {:else if items.length === 0}<p>Queue is empty: every ready task is running or done.</p>
  {:else}
    <ol>
      {#each items as item (item.task_id)}
        <li><div><strong>{item.title}</strong><span class="id">{item.task_id} · priority {item.priority}</span></div>
          <p><span aria-hidden="true">{reasons[item.reason].icon}</span> <strong>{reasons[item.reason].label}.</strong> {item.detail}</p></li>
      {/each}
    </ol>
  {/if}
</section>

<style>
  section { display: grid; gap: .75rem; padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; } h2 { margin: 0; } h2 span { color: #aeb5ad; font-size: .9rem; font-weight: 400; }
  ol { display: grid; gap: .6rem; margin: 0; padding-left: 1.4rem; } li { padding: .5rem .6rem; border: 1px solid #394139; } li div { display: flex; flex-wrap: wrap; justify-content: space-between; gap: .4rem; } .id { color: #aeb5ad; font: .8rem ui-monospace, monospace; } li p { margin: .3rem 0 0; color: #d8c98a; font-size: .9rem; } .bad { color: #ffb0b0; }
</style>
