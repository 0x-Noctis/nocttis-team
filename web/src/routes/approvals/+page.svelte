<script lang="ts">
  import { onMount } from 'svelte';
  import { approvalsApi, asApiError, runApi, taskApi } from '$lib/api/client';
  import type { ApiError } from '$lib/api/types';
  import { startPoller } from '$lib/realtime/run-poller';
  import { ConflictCard, DecisionHistory, PlanApprovalCard, isStale, type ApprovalsView, type AttentionTask, type PendingPlan } from '$lib/components/approval';

  const POLL_MS = 5000;

  let view = $state<ApprovalsView | null>(null);
  let loading = $state(true);
  let error = $state<ApiError | null>(null);
  let liveError = $state('');
  let actor = $state('');
  let notice = $state('');
  let busy = $state<Record<string, boolean>>({});
  let cardErrors = $state<Record<string, string>>({});

  const pendingCount = $derived(view?.pending_plans.length ?? 0);
  const attentionCount = $derived(view?.attention_tasks.length ?? 0);

  async function refresh(): Promise<undefined> {
    view = await approvalsApi.list();
  }

  // Keputusan basi (409) tidak boleh diam-diam diterapkan atau dianggap berhasil: jelaskan dan muat ulang antrean.
  const staleMessage = (subject: string) => `This ${subject} changed or was already decided by someone else, so your decision was not applied. The queue was refreshed.`;

  async function run(id: string, work: () => Promise<string>, subject: string) {
    busy[id] = true;
    delete cardErrors[id];
    notice = '';
    try {
      notice = await work();
      try { localStorage.setItem('noctis.actor', actor.trim()); } catch { /* penyimpanan lokal opsional */ }
    } catch (reason) {
      const problem = asApiError(reason);
      cardErrors[id] = isStale(problem) ? staleMessage(subject) : problem.error.message;
    } finally {
      busy[id] = false;
      await refresh().catch(() => undefined);
    }
  }

  const decidePlan = (plan: PendingPlan, decision: 'APPROVED' | 'REJECTED', reason: string) =>
    run(plan.plan_id, async () => {
      await runApi.decide(plan.run_id, { plan_id: plan.plan_id, actor_id: actor.trim(), decision, reason: reason || null });
      return `Plan ${plan.plan_id} ${decision === 'APPROVED' ? 'approved' : 'rejected'} as ${actor.trim()}.`;
    }, 'plan');

  const decideTask = (task: AttentionTask, action: 'retry' | 'cancel', reason: string) =>
    run(task.task_id, async () => {
      await (action === 'retry' ? approvalsApi.retryTask : approvalsApi.cancelTask)(task.task_id, task.version, actor.trim(), reason || null);
      return `Task ${task.task_id} ${action === 'retry' ? 'sent back to READY' : 'cancelled'} as ${actor.trim()}.`;
    }, 'task');

  onMount(() => {
    try { actor = localStorage.getItem('noctis.actor') ?? ''; } catch { /* abaikan */ }
    let poller: ReturnType<typeof startPoller> | undefined;
    let disposed = false;
    const onVisible = () => { if (!document.hidden) poller?.refresh(); };
    refresh()
      .then(() => {
        if (disposed) return;
        poller = startPoller(refresh, {
          intervalMs: POLL_MS,
          isVisible: () => !document.hidden,
          onError: (_reason, failures) => { liveError = `Live updates interrupted (attempt ${failures}). Retrying automatically.`; },
          onRecover: () => { liveError = ''; }
        });
        document.addEventListener('visibilitychange', onVisible);
        window.addEventListener('online', onVisible);
      })
      .catch((reason) => { if (!disposed) error = asApiError(reason); })
      .finally(() => { if (!disposed) loading = false; });
    return () => {
      disposed = true;
      poller?.stop();
      document.removeEventListener('visibilitychange', onVisible);
      window.removeEventListener('online', onVisible);
    };
  });
</script>

<svelte:head><title>Approvals · Noctis</title></svelte:head>
<main>
  <nav aria-label="Primary"><a href="/projects">Projects</a><a href="/tasks">Tasks</a><a href="/approvals" aria-current="page">Approvals</a><a href="/providers">Providers</a></nav>
  <header><p class="eyebrow">Human decisions</p><h1>Approvals</h1>
    <p>{#if view}{pendingCount} plan{pendingCount === 1 ? '' : 's'} waiting for approval · {attentionCount} task{attentionCount === 1 ? '' : 's'} need{attentionCount === 1 ? 's' : ''} a person.{:else}Plans and tasks that need a person.{/if}</p>
  </header>

  <section class="actor" aria-labelledby="actor-title">
    <h2 id="actor-title">Acting as</h2>
    <label>Your name or ID<input bind:value={actor} name="actor" autocomplete="username" maxlength="128" aria-describedby="actor-hint" /></label>
    <p id="actor-hint">Every decision below is recorded under this name in the audit trail. It is required before you can decide anything.</p>
  </section>

  {#if error}<section class="error" role="alert"><strong>{error.error.message}</strong><span>Request ID: <code>{error.error.request_id}</code></span></section>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  {#if liveError}<p class="warning" role="status">{liveError}</p>{/if}

  {#if loading}<p class="state" aria-live="polite" aria-busy="true">Loading the queue…</p>
  {:else if view}
    <section aria-labelledby="plans-title"><h2 id="plans-title">Plans waiting for approval</h2>
      {#if view.pending_plans.length === 0}<p class="state">Nothing is waiting for approval.</p>
      {:else}<div class="cards">{#each view.pending_plans as plan (plan.plan_id)}
        <PlanApprovalCard {plan} {actor} busy={!!busy[plan.plan_id]} error={cardErrors[plan.plan_id] ?? ''} onrefresh={() => refresh().catch(() => undefined)}
          onapprove={(reason) => decidePlan(plan, 'APPROVED', reason)} onreject={(reason) => decidePlan(plan, 'REJECTED', reason)} />
      {/each}</div>{/if}
    </section>

    <section aria-labelledby="tasks-title"><h2 id="tasks-title">Tasks that need a person</h2>
      {#if view.attention_tasks.length === 0}<p class="state">No task is waiting for a person.</p>
      {:else}<div class="cards">{#each view.attention_tasks as task (task.task_id)}
        <ConflictCard {task} {actor} busy={!!busy[task.task_id]} error={cardErrors[task.task_id] ?? ''} loadDiff={(id) => taskApi.diff(id)} onrefresh={() => refresh().catch(() => undefined)}
          onretry={(reason) => decideTask(task, 'retry', reason)} oncancel={(reason) => decideTask(task, 'cancel', reason)} />
      {/each}</div>{/if}
    </section>

    <DecisionHistory plans={view.plan_decisions} tasks={view.task_decisions} />
  {/if}
</main>

<style>
  :global(*) { box-sizing: border-box; } :global(body) { margin: 0; background: #090b09; color: #eef2ec; font-family: system-ui, sans-serif; } main { display: grid; gap: 1.5rem; max-width: 76rem; margin: auto; padding: 2rem; }
  nav { display: flex; flex-wrap: wrap; gap: 1.2rem; } a { color: #90e0a8; } a[aria-current='page'] { color: #fff; font-weight: 800; } h1 { margin: 0; font-size: clamp(2.5rem, 7vw, 4.5rem); } h2 { margin: 0 0 .75rem; } .eyebrow { margin: 0; color: #90e0a8; font: 700 .8rem monospace; text-transform: uppercase; }
  .actor { display: grid; gap: .5rem; padding: 1rem 1.25rem; border: 1px solid #394139; background: #121512; } .actor h2 { margin: 0; font-size: 1rem; } .actor p { margin: 0; color: #aeb5ad; font-size: .85rem; } label { display: grid; gap: .35rem; max-width: 24rem; font-weight: 700; }
  input { min-height: 2.6rem; padding: .5rem .7rem; border: 1px solid #525b52; background: #090b0a; color: inherit; font: inherit; } input:focus-visible, a:focus-visible { outline: 3px solid #f4d35e; outline-offset: 3px; }
  .cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 26rem), 1fr)); gap: 1rem; align-items: start; }
  .error, .notice, .warning, .state { margin: 0; padding: 1rem; border: 1px solid #697169; } .error { display: grid; gap: .4rem; border-color: #ff8b8b; color: #ffb0b0; } .notice { border-color: #65a978; } .warning { border-color: #d8c98a; color: #f2e6aa; }
  @media (max-width: 700px) { main { padding: 1rem; } }
</style>
