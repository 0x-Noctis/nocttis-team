<script lang="ts">
  import TaskStatusBadge from '../task/TaskStatusBadge.svelte';
  import TokenGauge from '../budget/TokenGauge.svelte';
  import { heartbeatState } from './helpers';
  import type { SlotState, WorkerSlot } from './types';

  let { slot, staleAfterSeconds = 60 }: { slot: WorkerSlot; staleAfterSeconds?: number } = $props();

  // Status berupa ikon + teks + penjelasan; tidak bergantung pada warna.
  const states: Record<SlotState, { icon: string; label: string; hint: string }> = {
    idle: { icon: '○', label: 'Idle', hint: 'Waiting for a task it may start.' },
    running: { icon: '▶', label: 'Running', hint: 'Working on its task.' },
    pausing: { icon: '‖', label: 'Pausing', hint: 'Pause requested; finishing the current step. No new task will start.' },
    paused: { icon: '‖', label: 'Paused', hint: 'Stopped at a checkpoint. Resume the run to continue.' },
    cancelling: { icon: '×', label: 'Cancelling', hint: 'Cancel requested; stopping the worker and releasing its file leases.' },
    draining: { icon: '⇣', label: 'Draining', hint: 'Shutting down: finishing current work and not taking new tasks.' }
  };
  const beat = $derived(heartbeatState(slot.heartbeat_age_seconds, staleAfterSeconds));
  const beatText = $derived(
    beat === 'none' ? 'No heartbeat yet'
    : beat === 'stale' ? `Heartbeat stale: ${slot.heartbeat_age_seconds}s ago. Claim may be recovered.`
    : beat === 'slow' ? `Heartbeat slow: ${slot.heartbeat_age_seconds}s ago`
    : `Heartbeat ${slot.heartbeat_age_seconds}s ago`
  );
</script>

<article class="slot {slot.state}" aria-label={`Worker slot ${slot.slot}: ${states[slot.state].label}`}>
  <header>
    <h3>Slot {slot.slot}</h3>
    <strong class="state"><span aria-hidden="true">{states[slot.state].icon}</span> {states[slot.state].label}</strong>
  </header>
  <p class="hint">{states[slot.state].hint}</p>
  {#if slot.task_id}
    <div class="task">
      <p class="id">{slot.task_id}</p>
      <p class="title">{slot.title}</p>
      {#if slot.task_status}<TaskStatusBadge status={slot.task_status} />{/if}
      {#if slot.attempt}<p class="meta">Attempt {slot.attempt.number} of {slot.attempt.max} · <code>{slot.attempt.branch}</code></p>{/if}
      <p class="beat {beat}" role={beat === 'stale' ? 'status' : undefined}>{beatText}</p>
    </div>
    <section aria-label={`Slot ${slot.slot} file leases`}>
      <h4>File leases</h4>
      {#if slot.leases.length}<ul>{#each slot.leases as lease (lease.pattern)}<li><code>{lease.pattern}</code> <span>expires in {lease.expires_in_seconds}s</span></li>{/each}</ul>{:else}<p class="none">None held</p>{/if}
    </section>
    {#if slot.tokens}<TokenGauge gauge={slot.tokens} compact />{/if}
  {:else}
    <p class="none">No task assigned.</p>
  {/if}
</article>

<style>
  article { display: grid; gap: .6rem; align-content: start; padding: 1rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  article.paused, article.pausing, article.draining { border-style: dashed; border-color: #d8c98a; } article.cancelling { border-style: dashed; border-color: #ff8b8b; }
  header { display: flex; justify-content: space-between; gap: .5rem; align-items: baseline; } h3, h4 { margin: 0; } h4 { font-size: .85rem; color: #aeb5ad; } .state { font: 700 .85rem ui-monospace, monospace; text-transform: uppercase; }
  .hint, .none, .meta { margin: 0; color: #aeb5ad; font-size: .85rem; } .id { margin: 0; color: #90e0a8; font: 700 .75rem ui-monospace, monospace; } .title { margin: 0; font-weight: 700; }
  .task { display: grid; gap: .4rem; } .beat { margin: 0; font-size: .85rem; } .beat.slow { color: #d8c98a; } .beat.stale { color: #ffb0b0; font-weight: 700; }
  ul { margin: 0; padding-left: 1rem; font-size: .85rem; } li span { color: #aeb5ad; }
</style>
