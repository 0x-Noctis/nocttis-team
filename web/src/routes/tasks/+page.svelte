<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiRequestError, taskApi, type TaskResponse } from '$lib/api/client';
  import type { ApiError, TaskContract } from '$lib/api/types';

  let tasks = $state<TaskResponse[]>([]);
  let loading = $state(true);
  let submitting = $state(false);
  let error = $state<ApiError | null>(null);
  let notice = $state('');
  let showForm = $state(false);

  const text = (form: FormData, name: string) => String(form.get(name) ?? '').trim();
  const lines = (form: FormData, name: string) =>
    text(form, name).split('\n').map((value) => value.trim()).filter(Boolean);
  const integer = (form: FormData, name: string) => Number(text(form, name));

  function asError(reason: unknown): ApiError {
    if (reason instanceof ApiRequestError) return reason.envelope;
    return { error: { code: 'NETWORK_ERROR', message: reason instanceof Error ? reason.message : 'Request failed.', details: {}, request_id: 'unavailable' } };
  }

  async function load() {
    loading = true;
    error = null;
    try { tasks = (await taskApi.list()).items; }
    catch (reason) { error = asError(reason); }
    finally { loading = false; }
  }

  async function create(event: SubmitEvent) {
    event.preventDefault();
    submitting = true;
    error = null;
    notice = '';
    const form = new FormData(event.currentTarget as HTMLFormElement);
    const contract: TaskContract = {
      id: text(form, 'id'), project_id: text(form, 'project_id'), project_run_id: text(form, 'project_run_id'),
      title: text(form, 'title'), role: text(form, 'role'), objective: text(form, 'objective'),
      depends_on: lines(form, 'depends_on'), allowed_paths: lines(form, 'allowed_paths'),
      context_refs: lines(form, 'context_refs'), acceptance_criteria: lines(form, 'acceptance_criteria'),
      verification_commands: lines(form, 'verification_commands'),
      limits: { max_input_tokens: integer(form, 'max_input_tokens'), max_output_tokens: integer(form, 'max_output_tokens'), max_tool_calls: integer(form, 'max_tool_calls'), max_attempts: integer(form, 'max_attempts'), timeout_seconds: integer(form, 'timeout_seconds') }
    };
    try {
      const created = await taskApi.create(contract);
      tasks = [...tasks, created].sort((left, right) => left.contract.id.localeCompare(right.contract.id));
      notice = `Task ${created.contract.id} created.`;
      showForm = false;
    } catch (reason) { error = asError(reason); }
    finally { submitting = false; }
  }

  onMount(load);
</script>

<svelte:head><title>Tasks · Noctis</title></svelte:head>
<main>
  <header><div><p class="eyebrow">Control plane</p><h1>Tasks</h1><p>Create manual work and follow runtime status.</p></div><button type="button" onclick={() => showForm = !showForm}>{showForm ? 'Close form' : 'Create task'}</button></header>
  {#if error}<section class="error" role="alert"><strong>{error.error.message}</strong><span>Request ID: <code>{error.error.request_id}</code></span></section>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  {#if showForm}
    <form onsubmit={create} aria-label="Create task">
      <h2>Manual task</h2>
      <div class="grid">
        <label>Task ID<input name="id" required /></label><label>Title<input name="title" required /></label>
        <label>Project UUID<input name="project_id" required /></label><label>Project run UUID<input name="project_run_id" required /></label>
        <label>Role<input name="role" required /></label><label>Objective<textarea name="objective" required></textarea></label>
        <label>Allowed paths, one per line<textarea name="allowed_paths" required></textarea></label><label>Acceptance criteria, one per line<textarea name="acceptance_criteria" required></textarea></label>
        <label>Verification commands, one per line<textarea name="verification_commands" required></textarea></label><label>Dependencies, one per line<textarea name="depends_on"></textarea></label>
        <label>Context refs, one per line<textarea name="context_refs"></textarea></label>
      </div>
      <fieldset><legend>Limits</legend><label>Input tokens<input name="max_input_tokens" type="number" min="1" value="10000" required /></label><label>Output tokens<input name="max_output_tokens" type="number" min="1" value="4000" required /></label><label>Tool calls<input name="max_tool_calls" type="number" min="1" value="20" required /></label><label>Attempts<input name="max_attempts" type="number" min="1" max="10" value="2" required /></label><label>Timeout seconds<input name="timeout_seconds" type="number" min="1" value="900" required /></label></fieldset>
      <button disabled={submitting}>{submitting ? 'Creating…' : 'Create task'}</button>
    </form>
  {/if}
  {#if loading}<p class="state" aria-live="polite" aria-busy="true">Loading tasks…</p>
  {:else if tasks.length === 0}<p class="state">No tasks yet. Create first manual task.</p>
  {:else}<ul class="tasks">{#each tasks as task (task.contract.id)}<li><a href={`/tasks/${encodeURIComponent(task.contract.id)}`}><strong>{task.contract.title}</strong><span>{task.contract.id}</span><span>{task.status} · version {task.version}</span></a></li>{/each}</ul>{/if}
</main>

<style>
  :global(*) { box-sizing: border-box; } :global(body) { margin: 0; background: #090b09; color: #eef2ec; font-family: system-ui, sans-serif; } main { max-width: 76rem; margin: auto; padding: 2rem; } header { display: flex; justify-content: space-between; gap: 2rem; align-items: end; border-bottom: 1px solid #394139; padding-bottom: 1.5rem; } h1 { font-size: clamp(2.5rem, 7vw, 5rem); margin: 0; } .eyebrow { color: #90e0a8; font: 700 .8rem monospace; text-transform: uppercase; } button, input, textarea { font: inherit; } button { background: #90e0a8; color: #071008; border: 0; padding: .75rem 1rem; font-weight: 800; cursor: pointer; } button:disabled { opacity: .55; } button:focus-visible, input:focus-visible, textarea:focus-visible, a:focus-visible { outline: 3px solid #f4d35e; outline-offset: 3px; } form { margin: 2rem 0; padding: 1.5rem; border: 1px solid #566056; background: #121512; } .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 1rem; } label { display: grid; gap: .4rem; color: #cbd2ca; } input, textarea { width: 100%; padding: .7rem; color: #fff; background: #080a08; border: 1px solid #697169; } textarea { min-height: 6rem; } fieldset { display: flex; flex-wrap: wrap; gap: 1rem; margin: 1rem 0; border: 1px solid #394139; } .tasks { display: grid; grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr)); gap: 1rem; padding: 0; list-style: none; } .tasks a { display: grid; gap: .65rem; min-height: 9rem; padding: 1.2rem; border: 1px solid #394139; color: inherit; text-decoration: none; background: #121512; } .tasks a:hover { border-color: #90e0a8; } .tasks span { color: #aeb5ad; } .error, .notice, .state { margin: 1.5rem 0; padding: 1rem; border: 1px solid #697169; } .error { border-color: #ff8b8b; color: #ffb0b0; display: grid; gap: .4rem; } .notice { border-color: #65a978; } @media (max-width: 700px) { main { padding: 1rem; } header, .grid { display: grid; grid-template-columns: 1fr; } } @media (prefers-reduced-motion: reduce) { * { scroll-behavior: auto !important; } }
</style>
