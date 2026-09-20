<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { ApiRequestError, taskApi, type ArtifactResponse, type TaskEventResponse, type TaskResponse } from '$lib/api/client';
  import { taskEventStream } from '$lib/realtime/task-events';
  import type { ApiError } from '$lib/api/types';

  let task = $state<TaskResponse | null>(null);
  let events = $state<TaskEventResponse[]>([]);
  let artifacts = $state<ArtifactResponse[]>([]);
  let diff = $state('');
  let loading = $state(true);
  let acting = $state(false);
  let error = $state<ApiError | null>(null);
  let liveError = $state('');
  let notice = $state('');

  const taskId = $derived(page.params.id ?? '');
  const usage = $derived(events.reduce((total, event) => {
    const payload = event.payload;
    for (const key of ['input_tokens', 'cached_tokens', 'output_tokens', 'tool_calls', 'latency_ms'] as const) {
      const value = payload[key];
      if (typeof value === 'number' && Number.isSafeInteger(value) && value >= 0) total[key] += value;
    }
    return total;
  }, { input_tokens: 0, cached_tokens: 0, output_tokens: 0, tool_calls: 0, latency_ms: 0 }));
  const verification = $derived(events.filter(({ event_type, payload }) =>
    event_type.toLowerCase().includes('verif') || 'exit_code' in payload || 'passed' in payload
  ));

  function asError(reason: unknown): ApiError {
    if (reason instanceof ApiRequestError) return reason.envelope;
    return { error: { code: 'NETWORK_ERROR', message: reason instanceof Error ? reason.message : 'Request failed.', details: {}, request_id: 'unavailable' } };
  }

  function mergeEvent(event: TaskEventResponse) {
    if (events.some(({ id }) => id === event.id)) return;
    events = [...events, event].sort((left, right) => left.id - right.id);
    liveError = '';
    if (event.to_status) void taskApi.get(taskId).then((current) => task = current).catch((reason) => error = asError(reason));
  }

  async function load() {
    loading = true;
    error = null;
    try {
      const [loadedTask, loadedEvents, loadedArtifacts] = await Promise.all([
        taskApi.get(taskId), taskApi.events(taskId), taskApi.artifacts(taskId)
      ]);
      task = loadedTask;
      events = loadedEvents;
      artifacts = loadedArtifacts;
      try { diff = await taskApi.diff(taskId); }
      catch (reason) {
        if (!(reason instanceof ApiRequestError) || reason.envelope.error.code !== 'NOT_FOUND') throw reason;
      }
    } catch (reason) { error = asError(reason); }
    finally { loading = false; }
  }

  async function action(name: 'start' | 'cancel' | 'retry') {
    if (!task || (name === 'cancel' && !confirm(`Cancel task ${task.contract.id}?`))) return;
    acting = true;
    error = null;
    notice = '';
    try {
      task = await taskApi.action(task.contract.id, name, task.version);
      notice = `${name[0].toUpperCase()}${name.slice(1)} accepted at version ${task.version}.`;
    } catch (reason) { error = asError(reason); }
    finally { acting = false; }
  }

  const safeResult = (event: TaskEventResponse) => {
    const payload = event.payload;
    return ['passed', 'status', 'exit_code', 'estimated']
      .filter((key) => ['string', 'number', 'boolean'].includes(typeof payload[key]))
      .map((key) => `${key}: ${String(payload[key])}`)
      .join(' · ') || 'Result recorded.';
  };

  onMount(() => {
    let close = () => {};
    void load().then(() => {
      if (!error) close = taskEventStream(taskId, mergeEvent, () => liveError = 'Live connection interrupted. Browser is reconnecting.', events.map(({ id }) => id));
    });
    return () => close();
  });
</script>

<svelte:head><title>{task ? `${task.contract.id} · Tasks` : 'Task · Noctis'}</title></svelte:head>
<main>
  <nav aria-label="Breadcrumb"><a href="/tasks">Tasks</a><span aria-hidden="true">/</span><span>{taskId}</span></nav>
  {#if error}<section class="error" role="alert"><strong>{error.error.message}</strong><span>Request ID: <code>{error.error.request_id}</code></span></section>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  {#if liveError}<p class="warning" role="status">{liveError}</p>{/if}
  {#if loading}<p class="state" aria-live="polite" aria-busy="true">Loading task…</p>
  {:else if task}
    <header><div><p class="eyebrow">{task.contract.id}</p><h1>{task.contract.title}</h1><p>{task.contract.objective}</p></div><div class="status"><strong>{task.status}</strong><span>Version {task.version}</span></div></header>
    <section class="actions" aria-label="Task actions"><button disabled={acting} onclick={() => action('start')}>Start</button><button disabled={acting} onclick={() => action('retry')}>Retry</button><button class="danger-button" disabled={acting} onclick={() => action('cancel')}>Cancel…</button></section>
    <div class="layout">
      <section><h2>Contract</h2><dl><div><dt>Role</dt><dd>{task.contract.role}</dd></div><div><dt>Project</dt><dd>{task.contract.project_id}</dd></div><div><dt>Project run</dt><dd>{task.contract.project_run_id}</dd></div></dl>
        <h3>Acceptance criteria</h3><ul>{#each task.contract.acceptance_criteria as item}<li>{item}</li>{/each}</ul>
        <h3>Allowed paths</h3><ul>{#each task.contract.allowed_paths as item}<li><code>{item}</code></li>{/each}</ul>
        <h3>Verification commands</h3><ul>{#each task.contract.verification_commands as item}<li><code>{item}</code></li>{/each}</ul>
      </section>
      <section><h2>Usage</h2><dl class="metrics"><div><dt>Input</dt><dd>{usage.input_tokens.toLocaleString()}</dd></div><div><dt>Cached</dt><dd>{usage.cached_tokens.toLocaleString()}</dd></div><div><dt>Output</dt><dd>{usage.output_tokens.toLocaleString()}</dd></div><div><dt>Tool calls</dt><dd>{usage.tool_calls.toLocaleString()}</dd></div><div><dt>Latency ms</dt><dd>{usage.latency_ms.toLocaleString()}</dd></div></dl>{#if events.length === 0}<p>No usage recorded.</p>{/if}</section>
    </div>
    <section><h2>Live events</h2>{#if events.length === 0}<p>No events yet. Connection remains open.</p>{:else}<ol class="timeline">{#each events as event (event.id)}<li><span>#{event.id}</span><div><strong>{event.event_type}</strong><p>{event.from_status ?? '—'} → {event.to_status ?? '—'} · {event.actor}</p></div></li>{/each}</ol>{/if}</section>
    <section><h2>Verification results</h2>{#if verification.length === 0}<p>No verification result recorded.</p>{:else}<ul>{#each verification as result (result.id)}<li><strong>{result.event_type}</strong> — {safeResult(result)}</li>{/each}</ul>{/if}</section>
    <section><h2>Artifacts</h2>{#if artifacts.length === 0}<p>No artifacts recorded.</p>{:else}<ul class="artifacts">{#each artifacts as artifact (artifact.id)}<li><div><strong>{artifact.kind}</strong><span>{artifact.size_bytes.toLocaleString()} bytes · SHA-256 {artifact.sha256}</span></div><a href={taskApi.artifactUrl(task.contract.id, artifact.id)}>Download</a></li>{/each}</ul>{/if}</section>
    <section><h2>Diff</h2>{#if diff}<pre aria-label="Task diff">{diff}</pre>{:else}<p>No diff artifact recorded.</p>{/if}</section>
  {:else}<p class="state">Task not found.</p>{/if}
</main>

<style>
  :global(*) { box-sizing: border-box; } :global(body) { margin: 0; background: #090b09; color: #eef2ec; font-family: system-ui, sans-serif; } main { max-width: 76rem; margin: auto; padding: 2rem; } nav { display: flex; gap: .6rem; color: #aeb5ad; } a { color: #90e0a8; } header { display: flex; justify-content: space-between; gap: 2rem; align-items: end; padding: 3rem 0 1.5rem; border-bottom: 1px solid #394139; } h1 { margin: 0; font-size: clamp(2.5rem, 7vw, 5rem); } h2 { margin-top: 0; } .eyebrow { color: #90e0a8; font: 700 .85rem monospace; } .status { display: grid; gap: .35rem; min-width: 10rem; padding: 1rem; border: 1px solid #7aa7d8; background: #101d29; } .actions { display: flex; gap: .75rem; margin: 1.5rem 0; padding: 0; border: 0; } button { padding: .7rem 1rem; border: 0; background: #90e0a8; color: #071008; font: 800 1rem system-ui; cursor: pointer; } .danger-button { background: #ffb0b0; } button:disabled { opacity: .55; } button:focus-visible, a:focus-visible, pre:focus-visible { outline: 3px solid #f4d35e; outline-offset: 3px; } section { margin: 1rem 0; padding: 1.25rem; border: 1px solid #394139; background: #121512; } .layout { display: grid; grid-template-columns: 1.4fr 1fr; gap: 1rem; } dl { display: grid; gap: .7rem; } dl div { display: grid; gap: .2rem; } dt { color: #aeb5ad; } dd { margin: 0; overflow-wrap: anywhere; } .metrics { grid-template-columns: repeat(2, minmax(0, 1fr)); } .metrics div { padding: .7rem; border: 1px solid #394139; } .metrics dd { font-size: 1.3rem; font-weight: 800; } .timeline, .artifacts { display: grid; gap: .8rem; padding: 0; list-style: none; } .timeline li, .artifacts li { display: flex; justify-content: space-between; gap: 1rem; padding: .8rem; border-left: 3px solid #7aa7d8; } .timeline p, .artifacts span { display: block; margin: .25rem 0 0; color: #aeb5ad; } pre { max-height: 34rem; overflow: auto; padding: 1rem; background: #070907; border: 1px solid #394139; white-space: pre; } .error, .notice, .warning, .state { padding: 1rem; border: 1px solid #697169; } .error { display: grid; gap: .4rem; border-color: #ff8b8b; color: #ffb0b0; } .notice { border-color: #65a978; } .warning { border-color: #d8c98a; color: #f2e6aa; } @media (max-width: 700px) { main { padding: 1rem; } header, .layout { display: grid; grid-template-columns: 1fr; } .actions { flex-wrap: wrap; } } @media (prefers-reduced-motion: reduce) { * { scroll-behavior: auto !important; } }
</style>
