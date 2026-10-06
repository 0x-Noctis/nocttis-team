<script lang="ts">
  import ConfirmAction from './ConfirmAction.svelte';
  import { describeEvent, explainError, validateDecision } from './helpers';
  import type { AttentionTask } from './types';

  let {
    task,
    actor,
    busy = false,
    error = '',
    loadDiff,
    onretry,
    oncancel,
    onrefresh
  }: {
    task: AttentionTask;
    actor: string;
    busy?: boolean;
    error?: string;
    loadDiff: (taskId: string) => Promise<string>;
    onretry: (reason: string) => void;
    oncancel: (reason: string) => void;
    onrefresh?: () => void;
  } = $props();

  let diff = $state('');
  let diffError = $state('');
  let diffLoading = $state(false);
  const actorProblem = $derived(validateDecision(actor, '', false));
  const who = $derived(actor.trim() || '(no actor)');

  async function showDiff() {
    diffLoading = true;
    diffError = '';
    try { diff = await loadDiff(task.task_id); }
    catch (reason) { diffError = reason instanceof Error ? reason.message : 'Diff unavailable.'; }
    finally { diffLoading = false; }
  }
</script>

<article aria-labelledby={`task-${task.task_id}`}>
  <header>
    <div><p class="eyebrow">{task.project_name} · {task.run_objective}</p><h3 id={`task-${task.task_id}`}>{task.title}</h3><p class="id">{task.task_id}</p></div>
    <strong class="status"><span aria-hidden="true">{task.status === 'NEEDS_HUMAN' ? '?' : '!'}</span> {task.status === 'NEEDS_HUMAN' ? 'Needs a human' : 'Conflict'}</strong>
  </header>
  <p class="explain">{task.status === 'CONFLICT' ? 'The patch conflicted with others during integration and is waiting to be handed over to a person. ' : ''}{explainError(task.last_error)}</p>

  <section aria-label={`Recent activity for ${task.task_id}`}>
    <h4>Recent activity</h4>
    {#if task.recent_events.length}<ol>{#each task.recent_events as event (event.id)}<li><time datetime={event.created_at}>{event.created_at.slice(11, 19)}</time> {describeEvent(event)}</li>{/each}</ol>{:else}<p class="none">No activity recorded.</p>{/if}
  </section>

  <section aria-label={`Changes for ${task.task_id}`}>
    <h4>Changes</h4>
    {#if !task.has_patch}<p class="none">No patch was saved for this task.</p>
    {:else if diff}<!-- Diff panjang bisa di-scroll; blok itu harus dapat difokus keyboard agar terbaca tanpa mouse (WCAG 2.1.1). -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <pre tabindex="0" aria-label={`Diff for ${task.task_id}`}>{diff}</pre>
    {:else}<button type="button" onclick={showDiff} disabled={diffLoading}>{diffLoading ? 'Loading diff…' : 'Show diff'}</button>{/if}
    {#if diffError}<p class="error" role="alert">{diffError}</p>{/if}
    <p><a href={`/tasks/${encodeURIComponent(task.task_id)}`}>Open the task page for artifacts and full history</a></p>
  </section>

  <div class="actions">
    {#if task.can_retry}
      <ConfirmAction label="Retry task" confirmLabel="Yes, retry" tone="risky" reasonRequired reasonLabel="Why is retrying safe?" {busy}
        description={`A new attempt starts from a clean copy. The earlier attempt’s side effects are unknown, so only retry if you checked them. This is recorded as ${who}.`}
        disabled={!!actorProblem} disabledReason={actorProblem ?? ''} validate={(reason) => validateDecision(actor, reason, true)} onconfirm={onretry} />
    {:else}
      <p class="none">Retry is unavailable until the integrator hands this task to a person.</p>
    {/if}
    <ConfirmAction label="Cancel task" confirmLabel="Yes, cancel task" tone="destructive" reasonRequired reasonLabel="Reason for cancelling" {busy}
      description={`Cancelling stops this task for good; tasks that depend on it stay blocked. This is recorded as ${who}.`}
      disabled={!!actorProblem} disabledReason={actorProblem ?? ''} validate={(reason) => validateDecision(actor, reason, true)} onconfirm={oncancel} />
  </div>
  {#if error}<p class="error" role="alert">{error}{#if onrefresh} <button type="button" class="link" onclick={onrefresh}>Refresh the queue</button>{/if}</p>{/if}
</article>

<style>
  article { display: grid; gap: .75rem; padding: 1.1rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  header { display: flex; flex-wrap: wrap; justify-content: space-between; gap: .75rem; } h3 { margin: 0; font-size: 1.15rem; } h4 { margin: 0 0 .35rem; color: #aeb5ad; font-size: .85rem; } .id { margin: .15rem 0 0; color: #aeb5ad; font: .8rem ui-monospace, monospace; }
  .eyebrow { margin: 0 0 .25rem; color: #90e0a8; font: 700 .75rem ui-monospace, monospace; text-transform: uppercase; } .status { font: 700 .85rem ui-monospace, monospace; text-transform: uppercase; color: #f2e6aa; }
  .explain { margin: 0; padding: .6rem .8rem; border-left: 3px solid #d8c98a; } ol { display: grid; gap: .3rem; margin: 0; padding-left: 1.2rem; font-size: .88rem; } time { color: #aeb5ad; font-family: ui-monospace, monospace; } .none { margin: 0; color: #aeb5ad; }
  pre { max-height: 22rem; overflow: auto; margin: 0; padding: .8rem; border: 1px solid #394139; background: #070907; white-space: pre; } a { color: #90e0a8; }
  button { min-height: 2.5rem; padding: .5rem 1rem; border: 1px solid #525b52; background: #202420; color: inherit; font-weight: 800; cursor: pointer; } button:disabled { opacity: .6; }
  .actions { display: flex; flex-wrap: wrap; gap: .75rem; align-items: start; } .error { margin: 0; color: #ffb0b0; } .link { min-height: 0; padding: 0; border: 0; background: none; color: #90e0a8; text-decoration: underline; font: inherit; font-weight: 400; }
  a:focus-visible, button:focus-visible, pre:focus-visible { outline: 3px solid #f4d35e; outline-offset: 3px; }
</style>
