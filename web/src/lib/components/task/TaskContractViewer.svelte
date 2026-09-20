<script lang="ts">
  import type { TaskContract } from '$lib/api/types';
  import TaskStatusBadge from './TaskStatusBadge.svelte';

  let { task, loading = false, error = '' }: { task?: TaskContract; loading?: boolean; error?: string } = $props();
</script>

{#if loading}
  <section class="state" aria-live="polite" aria-busy="true"><h2>Loading task contract</h2><p>Reading task requirements…</p></section>
{:else if error}
  <section class="state error" role="alert"><h2>Task contract unavailable</h2><p>{error}</p></section>
{:else if !task}
  <section class="state"><h2>No task selected</h2><p>Select a task to inspect its contract.</p></section>
{:else}
  <article aria-labelledby={`task-${task.id}`}>
    <header><div><p class="eyebrow">{task.id}</p><h2 id={`task-${task.id}`}>{task.title}</h2></div><TaskStatusBadge status={task.status} /></header>
    <p>{task.description}</p>
    <div class="grid">
      <section><h3>Acceptance criteria</h3><ul>{#each task.acceptance_criteria as criterion}<li>{criterion}</li>{/each}</ul></section>
      <section><h3>Allowed paths</h3><ul>{#each task.allowed_paths as path}<li><code>{path}</code></li>{/each}</ul></section>
      <section><h3>Verification</h3><ul>{#each task.verification_commands as command}<li><code>{command}</code></li>{/each}</ul></section>
      <section><h3>Dependencies</h3>{#if task.dependencies.length}<ul>{#each task.dependencies as dependency}<li>{dependency}</li>{/each}</ul>{:else}<p>None</p>{/if}<p>Attempt limit: {task.attempt_limit}</p></section>
    </div>
  </article>
{/if}

<style>
  article, .state { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  header { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 1rem; }
  h2, h3, p { margin-top: 0; } .eyebrow { margin-bottom: .35rem; color: #90e0a8; font: 700 .78rem ui-monospace, monospace; }
  .grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; }
  section { min-width: 0; } ul { padding-left: 1.25rem; } li + li { margin-top: .45rem; } code { overflow-wrap: anywhere; color: #c6d8c9; }
  .error { border-color: #ff8b8b; }
  @media (max-width: 680px) { .grid { grid-template-columns: 1fr; } }
</style>
