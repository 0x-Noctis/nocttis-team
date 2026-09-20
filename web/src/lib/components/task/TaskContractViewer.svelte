<script lang="ts">
  import { isValidTaskLimits, type TaskView } from '$lib/api/types';
  import TaskStatusBadge from './TaskStatusBadge.svelte';

  let { task, loading = false, error = '' }: { task?: TaskView; loading?: boolean; error?: string } = $props();
</script>

{#if loading}
  <section class="state" aria-live="polite" aria-busy="true"><h2>Loading task contract</h2><p>Reading task requirements…</p></section>
{:else if error}
  <section class="state error" role="alert"><h2>Task contract unavailable</h2><p>{error}</p></section>
{:else if !task}
  <section class="state"><h2>No task selected</h2><p>Select a task to inspect its contract.</p></section>
{:else}
  <article aria-labelledby={`task-${task.contract.id}`}>
    <header><div><p class="eyebrow">{task.contract.id} · {task.contract.project_id}</p><h2 id={`task-${task.contract.id}`}>{task.contract.title}</h2></div><TaskStatusBadge status={task.status} /></header>
    <p><strong>Role:</strong> {task.contract.role}</p>
    <p>{task.contract.objective}</p>
    <div class="grid">
      <section><h3>Acceptance criteria</h3><ul>{#each task.contract.acceptance_criteria as criterion}<li>{criterion}</li>{/each}</ul></section>
      <section><h3>Allowed paths</h3><ul>{#each task.contract.allowed_paths as path}<li><code>{path}</code></li>{/each}</ul></section>
      <section><h3>Verification</h3><ul>{#each task.contract.verification_commands as command}<li><code>{command}</code></li>{/each}</ul></section>
      <section><h3>Dependencies</h3>{#if task.contract.depends_on.length}<ul>{#each task.contract.depends_on as dependency}<li>{dependency}</li>{/each}</ul>{:else}<p>None</p>{/if}</section>
      <section><h3>Context references</h3>{#if task.contract.context_refs.length}<ul>{#each task.contract.context_refs as reference}<li><code>{reference}</code></li>{/each}</ul>{:else}<p>None</p>{/if}</section>
      <section><h3>Limits</h3>{#if isValidTaskLimits(task.contract.limits)}<dl><div><dt>Input tokens</dt><dd>{task.contract.limits.max_input_tokens.toLocaleString()}</dd></div><div><dt>Output tokens</dt><dd>{task.contract.limits.max_output_tokens.toLocaleString()}</dd></div><div><dt>Tool calls</dt><dd>{task.contract.limits.max_tool_calls}</dd></div><div><dt>Attempts</dt><dd>{task.contract.limits.max_attempts}</dd></div><div><dt>Timeout</dt><dd>{task.contract.limits.timeout_seconds.toLocaleString()} seconds</dd></div></dl>{:else}<p class="error" role="alert">Task limits are invalid.</p>{/if}</section>
    </div>
  </article>
{/if}

<style>
  article, .state { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  header { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 1rem; }
  h2, h3, p { margin-top: 0; } .eyebrow { margin-bottom: .35rem; color: #90e0a8; font: 700 .78rem ui-monospace, monospace; }
  .grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; }
  section { min-width: 0; } ul { padding-left: 1.25rem; } li + li { margin-top: .45rem; } code { overflow-wrap: anywhere; color: #c6d8c9; }
  dl { margin: 0; } dl div { display: flex; justify-content: space-between; gap: 1rem; } dd { margin: 0; font-weight: 700; }
  .error { border-color: #ff8b8b; }
  @media (max-width: 680px) { .grid { grid-template-columns: 1fr; } }
</style>
