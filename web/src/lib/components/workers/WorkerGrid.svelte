<script lang="ts">
  import WorkerSlotCard from './WorkerSlotCard.svelte';
  import type { WorkerSlot } from './types';

  let { slots, staleAfterSeconds = 60, loading = false, error = '' }: { slots: WorkerSlot[]; staleAfterSeconds?: number; loading?: boolean; error?: string } = $props();

  const active = $derived(slots.filter((slot) => slot.state !== 'idle').length);
</script>

<section aria-labelledby="workers-title">
  <h2 id="workers-title">Workers{#if slots.length} <span>({active} of {slots.length} active)</span>{/if}</h2>
  {#if loading}<p aria-live="polite" aria-busy="true">Loading workers…</p>
  {:else if error}<p class="bad" role="alert">Workers unavailable: {error}</p>
  {:else if slots.length === 0}<p>No worker slots are configured.</p>
  {:else}
    <div class="grid">{#each slots as slot (slot.slot)}<WorkerSlotCard {slot} {staleAfterSeconds} />{/each}</div>
  {/if}
</section>

<style>
  section { display: grid; gap: .75rem; color: #eef2ec; } h2 { margin: 0; } h2 span { color: #aeb5ad; font-size: .9rem; font-weight: 400; }
  /* 1 kolom di ponsel, 2 di tablet, sampai 4 di desktop; 2-4 slot tetap rapi tanpa scroll horizontal. */
  .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 15rem), 1fr)); gap: 1rem; } .bad { color: #ffb0b0; }
</style>
