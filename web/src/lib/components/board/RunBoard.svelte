<script lang="ts">
  import type { TaskView } from '$lib/api/types';
  import TaskStatusBadge from '../task/TaskStatusBadge.svelte';
  import { blockedBy, columnOf, indexTasks, type BoardColumn } from './graph';

  let {
    tasks,
    loading = false,
    error = '',
    onselect
  }: { tasks: TaskView[]; loading?: boolean; error?: string; onselect?: (id: string) => void } = $props();

  // Board berupa kolom statis (tanpa drag-and-drop): status berubah lewat orchestrator, bukan lewat UI.
  const columns: { key: BoardColumn; title: string }[] = [
    { key: 'blocked', title: 'Blocked' },
    { key: 'queued', title: 'Queued' },
    { key: 'in_progress', title: 'In progress' },
    { key: 'checking', title: 'Review & verify' },
    { key: 'attention', title: 'Needs attention' },
    { key: 'closed', title: 'Done / cancelled' }
  ];
  const byId = $derived(indexTasks(tasks));
  const grouped = $derived(columns.map((column) => ({ ...column, items: tasks.filter((task) => columnOf(task, byId) === column.key) })));
</script>

{#if loading}
  <section class="state" aria-live="polite" aria-busy="true"><h2>Loading board</h2><p>Reading run tasks…</p></section>
{:else if error}
  <section class="state error" role="alert"><h2>Board unavailable</h2><p>{error}</p></section>
{:else if !tasks.length}
  <section class="state"><h2>No tasks yet</h2><p>Tasks appear here once a plan is approved.</p></section>
{:else}
  <div class="board">
    {#each grouped as column (column.key)}
      <section aria-labelledby={`col-${column.key}`}>
        <h3 id={`col-${column.key}`}>{column.title} <span>({column.items.length})</span></h3>
        {#if column.items.length}
          <ul>
            {#each column.items as task (task.contract.id)}
              {@const blockers = blockedBy(task, byId)}
              <li>
                <p class="id">{task.contract.id}</p>
                <h4>{task.contract.title}</h4>
                <TaskStatusBadge status={task.status} />
                {#if blockers.length && column.key === 'blocked'}
                  <p class="blocked">Waiting for {blockers.map((b) => `${b.id} (${b.status === 'MISSING' ? 'missing' : b.status.toLowerCase()})`).join(', ')}</p>
                {/if}
                {#if onselect}<button type="button" onclick={() => onselect(task.contract.id)} aria-label={`Open task ${task.contract.id}`}>Open</button>{/if}
              </li>
            {/each}
          </ul>
        {:else}<p class="none">None</p>{/if}
      </section>
    {/each}
  </div>
{/if}

<style>
  .board { display: grid; grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr)); gap: 1rem; color: #eef2ec; }
  section { padding: 1rem; border: 1px solid #394139; background: #121512; } .state { color: #eef2ec; } .state.error { border-color: #ff8b8b; color: #ffb0b0; }
  h3 { margin: 0 0 .75rem; font-size: 1rem; } h3 span { color: #aeb5ad; } ul { display: grid; gap: .6rem; margin: 0; padding: 0; list-style: none; }
  li { display: grid; gap: .4rem; padding: .75rem; border: 1px solid #394139; background: #0c0e0c; } .id { margin: 0; color: #90e0a8; font: 700 .75rem ui-monospace, monospace; } h4 { margin: 0; }
  .blocked { margin: 0; color: #d8c98a; font-size: .85rem; } .none { margin: 0; color: #aeb5ad; }
  button { justify-self: start; min-height: 2.25rem; padding: .35rem .8rem; border: 1px solid #525b52; background: #202420; color: inherit; font-weight: 700; cursor: pointer; } button:focus-visible { outline: 3px solid #fff; outline-offset: 3px; }
</style>
