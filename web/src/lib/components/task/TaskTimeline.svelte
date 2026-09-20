<script lang="ts">
  import type { TaskTimelineEvent } from '$lib/api/types';
  import TaskStatusBadge from './TaskStatusBadge.svelte';

  let { events, loading = false, error = '' }: { events: TaskTimelineEvent[]; loading?: boolean; error?: string } = $props();
</script>

<section aria-labelledby="task-timeline-title">
  <h2 id="task-timeline-title">Task timeline</h2>
  {#if loading}<p aria-live="polite" aria-busy="true">Loading timeline…</p>
  {:else if error}<p class="error" role="alert">Timeline unavailable: {error}</p>
  {:else if events.length === 0}<p>No task events yet.</p>
  {:else}<ol>{#each events as event (event.id)}<li><TaskStatusBadge status={event.status} /><div><strong>{event.label}</strong><time datetime={event.timestamp}>{new Date(event.timestamp).toLocaleString()}</time><p>{event.detail}</p></div></li>{/each}</ol>{/if}
</section>

<style>
  section { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; } h2 { margin-top: 0; }
  ol { display: grid; gap: 1rem; margin: 0; padding: 0; list-style: none; } li { display: grid; grid-template-columns: auto 1fr; gap: 1rem; padding-left: .8rem; border-left: 3px solid #697169; }
  strong, time { display: block; } time { margin-top: .25rem; color: #aeb5ad; font-size: .85rem; } li p { margin-bottom: 0; } .error { color: #ffb0b0; }
  @media (max-width: 560px) { li { grid-template-columns: 1fr; } }
</style>
