<script lang="ts">
  import type { TaskView } from '$lib/api/types';
  import TaskStatusBadge from '../task/TaskStatusBadge.svelte';
  import { dependencyLevels } from './graph';

  let { tasks, loading = false, error = '' }: { tasks: TaskView[]; loading?: boolean; error?: string } = $props();

  const graph = $derived(dependencyLevels(tasks));
</script>

<section aria-labelledby="dag-title">
  <h2 id="dag-title">Dependency order</h2>
  {#if loading}<p aria-live="polite" aria-busy="true">Loading dependency graph…</p>
  {:else if error}<p class="error" role="alert">Graph unavailable: {error}</p>
  {:else if !tasks.length}<p>No tasks in this plan.</p>
  {:else}
    <ol>
      {#each graph.levels as level, index}
        <li>
          <h3>Step {index + 1}{index === 0 ? ' · no dependencies' : ''}</h3>
          <ul>
            {#each level as task (task.contract.id)}
              <li><strong>{task.contract.id}</strong> {task.contract.title} <TaskStatusBadge status={task.status} />
                {#if task.contract.depends_on.length}<span class="deps">after {task.contract.depends_on.join(', ')}</span>{/if}</li>
            {/each}
          </ul>
        </li>
      {/each}
    </ol>
    {#if graph.unresolved.length}
      <div class="error" role="alert">
        <h3>Unresolved dependencies</h3>
        <p>These tasks reference a missing task or form a cycle, so they can never start:</p>
        <ul>{#each graph.unresolved as task (task.contract.id)}<li><strong>{task.contract.id}</strong> after {task.contract.depends_on.join(', ') || '—'}</li>{/each}</ul>
      </div>
    {/if}
  {/if}
</section>

<style>
  section { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; } h2 { margin-top: 0; } h3 { margin: 0 0 .4rem; font-size: 1rem; }
  ol { display: grid; gap: 1rem; margin: 0; padding-left: 1.2rem; } ul { display: grid; gap: .5rem; margin: 0; padding-left: 1rem; }
  .deps { color: #aeb5ad; font-size: .85rem; } .error { color: #ffb0b0; }
</style>
