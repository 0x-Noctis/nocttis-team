<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { asApiError, runApi, schedulerApi, type RunDetail, type SchedulerView } from '$lib/api/client';
  import type { ApiError, TaskView } from '$lib/api/types';
  import { startPoller } from '$lib/realtime/run-poller';
  import { DagSummary, RunBoard } from '$lib/components/board';
  import { RunBudgetPanel } from '$lib/components/budget';
  import { AttemptList, LeasePanel, QueuePanel, WorkerGrid, controlNotice, padSlots, uniqueBy } from '$lib/components/workers';
  import { BudgetMeter, PlanApprovalPanel, PlanReview, RunSummary, type PlanApprovalInput, type ProposedPlan, type RunStatus } from '$lib/components/project';

  const POLL_MS = 3000;
  const terminal: RunStatus[] = ['DONE', 'CANCELLED', 'FAILED'];

  let detail = $state<RunDetail | null>(null);
  let plans = $state<ProposedPlan[]>([]);
  let tasks = $state<TaskView[]>([]);
  let loading = $state(true);
  let acting = $state(false);
  let error = $state<ApiError | null>(null);
  let liveError = $state('');
  let notice = $state('');
  let decisionError = $state('');
  let confirmingCancel = $state(false);
  let actorId = $state('');
  let leading = $state(false);
  let scheduler = $state<SchedulerView | null>(null);
  let schedulerError = $state('');
  // Aksi kontrol yang sedang dikirim; tombol dikunci dan hasilnya ditunggu sampai server menjawab.
  let pendingAction = $state<'pause' | 'resume' | 'cancel' | null>(null);

  const runId = $derived(page.params.id ?? '');
  const run = $derived(detail?.run);
  // Plan yang menunggu keputusan menang; kalau tidak ada, tampilkan versi terbaru (API mengurutkan terbaru dulu).
  const plan = $derived(plans.find((candidate) => candidate.status === 'PROPOSED') ?? plans[0]);
  // Sebelum task dibuat (plan belum disetujui), DAG diperlihatkan dari isi plan sebagai preview.
  const dagTasks = $derived<TaskView[]>(tasks.length ? tasks : (plan?.tasks ?? []).map((contract) => ({ contract, status: 'PLANNED' })));
  const canPause = $derived(run?.status === 'RUNNING');
  const canResume = $derived(run?.status === 'PAUSED');
  // Lead hanya boleh dipanggil bila run masih merencanakan dan belum ada proposal yang menunggu keputusan.
  const canAskLead = $derived(
    (run?.status === 'PLANNING' || run?.status === 'AWAITING_APPROVAL') && !plans.some((candidate) => candidate.status === 'PROPOSED')
  );
  const errorLines = $derived(Object.entries(error?.error.details ?? {}).map(([key, value]) => `${key}: ${String(value)}`));
  const canCancel = $derived(run?.status === 'RUNNING' || run?.status === 'PAUSED');

  // Data scheduler diganti utuh tiap poll (bukan ditambahkan), dan kuncinya dijaga unik agar refresh atau
  // reconnect tidak pernah menggandakan baris.
  const slots = $derived(scheduler ? padSlots(scheduler.slots, scheduler.max_slots, scheduler.run_status) : []);
  const activeSlots = $derived(scheduler?.slots.length ?? 0);
  const queue = $derived(uniqueBy(scheduler?.queue ?? [], (item) => item.task_id));
  const leases = $derived(uniqueBy(scheduler?.leases ?? [], (lease) => `${lease.task_id}|${lease.pattern}`));
  const conflicts = $derived(uniqueBy(scheduler?.conflicts ?? [], (conflict) => `${conflict.task_id}|${conflict.pattern}`));
  const attempts = $derived(uniqueBy(scheduler?.attempts ?? [], (attempt) => `${attempt.task_id}|${attempt.number}`));
  const taskBudgets = $derived(uniqueBy(scheduler?.task_budgets ?? [], (gauge) => gauge.label));
  const showParallel = $derived(!!run && run.status !== 'PLANNING' && run.status !== 'AWAITING_APPROVAL');
  const controlText = $derived(run ? controlNotice(run.status, activeSlots) : '');

  // Satu siklus baca: run, plan, dan task diambil bersamaan lalu diganti sekaligus agar tampilan konsisten.
  async function refresh(): Promise<'stop' | undefined> {
    // Kegagalan membaca status scheduler tidak boleh mematikan seluruh halaman; ditampilkan di panelnya.
    const loadedScheduler = schedulerApi
      .view(runId)
      .then((view) => ({ view, message: '' }))
      .catch((reason) => ({ view: null, message: asApiError(reason).error.message }));
    const [loadedDetail, loadedPlans, loadedTasks, loadedView] = await Promise.all([runApi.get(runId), runApi.plans(runId), runApi.tasks(runId), loadedScheduler]);
    detail = loadedDetail;
    plans = loadedPlans;
    tasks = loadedTasks;
    scheduler = loadedView.view ?? scheduler;
    schedulerError = loadedView.message;
    // Berhenti polling hanya bila run selesai DAN semua worker sudah berhenti (cancel butuh waktu).
    return terminal.includes(loadedDetail.run.status) && (loadedView.view?.slots.length ?? 0) === 0 ? 'stop' : undefined;
  }

  async function act(work: () => Promise<string>) {
    acting = true;
    error = null;
    notice = '';
    try {
      notice = await work();
      await refresh().catch(() => undefined);
    } catch (reason) { error = asApiError(reason); }
    finally { acting = false; }
  }

  async function decide(approval: PlanApprovalInput) {
    decisionError = '';
    try { localStorage.setItem('noctis.actor', approval.actor_id); } catch { /* penyimpanan lokal opsional */ }
    acting = true;
    notice = '';
    try {
      await runApi.decide(runId, approval);
      notice = approval.decision === 'APPROVED' ? 'Plan approved. Tasks are being scheduled.' : 'Plan rejected.';
      await refresh().catch(() => undefined);
    } catch (reason) { decisionError = asApiError(reason).error.message; }
    finally { acting = false; }
  }

  async function askLead() {
    leading = true;
    error = null;
    notice = '';
    try {
      await runApi.leadPlan(runId);
      notice = 'Lead proposed a plan. Review it before approving.';
      await refresh().catch(() => undefined);
    } catch (reason) { error = asApiError(reason); }
    finally { leading = false; }
  }

  const transition = (action: 'pause' | 'resume' | 'cancel') => {
    const expected = run?.status;
    if (!expected) return;
    confirmingCancel = false;
    pendingAction = action;
    return act(async () => {
      await runApi.transition(runId, action, expected);
      return `Run ${action} accepted.`;
    }).finally(() => (pendingAction = null));
  };

  onMount(() => {
    try { actorId = localStorage.getItem('noctis.actor') ?? ''; } catch { /* abaikan */ }
    let poller: ReturnType<typeof startPoller> | undefined;
    let disposed = false;
    const onVisible = () => { if (!document.hidden) poller?.refresh(); };

    refresh()
      .then((result) => {
        if (disposed || result === 'stop') return;
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

<svelte:head><title>{run ? `${run.objective} · Runs` : 'Run · Noctis'}</title></svelte:head>
<main>
  <nav aria-label="Breadcrumb"><a href="/projects">Projects</a><span aria-hidden="true">/</span>{#if run}<a href={`/projects/${encodeURIComponent(run.project_id)}`}>Project</a><span aria-hidden="true">/</span>{/if}<span>Run {runId}</span></nav>
  <h1>Run details</h1>
  {#if error}<section class="error" role="alert"><strong>{error.error.message}</strong>{#each errorLines as line}<span>{line}</span>{/each}<span>Request ID: <code>{error.error.request_id}</code></span></section>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  {#if liveError}<p class="warning" role="status">{liveError}</p>{/if}

  {#if loading}<p class="state" aria-live="polite" aria-busy="true">Loading run…</p>
  {:else if run && detail}
    <RunSummary {run} />

    <section class="controls" aria-label="Run controls">
      <button type="button" disabled={acting || !canPause} onclick={() => transition('pause')}>Pause</button>
      <button type="button" disabled={acting || !canResume} onclick={() => transition('resume')}>Resume</button>
      {#if confirmingCancel}
        <span role="alert">Cancel this run? Running tasks will stop.</span>
        <button type="button" class="danger" disabled={acting} onclick={() => transition('cancel')}>Yes, cancel run</button>
        <button type="button" onclick={() => (confirmingCancel = false)}>Keep running</button>
      {:else}
        <button type="button" class="danger" disabled={acting || !canCancel} onclick={() => (confirmingCancel = true)}>Cancel run…</button>
      {/if}
      <span class="live">{terminal.includes(run.status) && activeSlots === 0 ? 'Run finished; updates stopped.' : `Live updates every ${POLL_MS / 1000} s`}</span>
    </section>
    {#if pendingAction}<p class="pending" role="status" aria-live="polite">Sending {pendingAction} request… waiting for the result.</p>{/if}
    {#if controlText}<p class="pending" role="status" aria-live="polite">{controlText}</p>{/if}

    <BudgetMeter budget={detail.budget} />

    {#if showParallel}
      <WorkerGrid {slots} error={schedulerError} />
      <QueuePanel items={queue} error={schedulerError} />
      <LeasePanel {leases} {conflicts} error={schedulerError} />
      <AttemptList {attempts} error={schedulerError} />
      <RunBudgetPanel title="Budget guard (live)" run={scheduler?.budget} tasks={taskBudgets} error={schedulerError} />
    {/if}

    {#if canAskLead}
      <section class="lead" aria-labelledby="lead-title"><h2 id="lead-title">Lead planner</h2>
        <p>Ask the Lead agent to draft a task plan from the objective and repository discovery. You still approve it before anything runs.</p>
        <button type="button" class="primary" disabled={leading || acting} aria-busy={leading} onclick={askLead}>{leading ? 'Lead is planning… this can take a minute' : plan ? 'Ask Lead for a new plan' : 'Ask Lead to plan'}</button>
      </section>
    {/if}

    {#if plan}
      <PlanReview {plan} availableTokens={detail.budget.limit - detail.budget.reserved} />
      <PlanApprovalPanel {plan} {actorId} submitting={acting} error={decisionError} onsubmit={decide} />
    {:else}
      <PlanReview />
    {/if}

    <DagSummary tasks={dagTasks} />
    <section aria-labelledby="board-title"><h2 id="board-title">Task board</h2>
      {#if tasks.length}<RunBoard {tasks} onselect={(id) => goto(`/tasks/${encodeURIComponent(id)}`)} />
      {:else}<p>Tasks appear here once a plan is approved.</p>{/if}
    </section>
  {:else if !error}<p class="state">Run not found.</p>{/if}
</main>

<style>
  h1 { margin: 0; font-size: clamp(1.8rem, 5vw, 2.6rem); }
  :global(*) { box-sizing: border-box; } :global(body) { margin: 0; background: #090b09; color: #eef2ec; font-family: system-ui, sans-serif; } main { display: grid; gap: 1.25rem; max-width: 76rem; margin: auto; padding: 2rem; }
  nav { display: flex; flex-wrap: wrap; gap: .6rem; color: #aeb5ad; } a { color: #90e0a8; } h2 { margin-top: 0; }
  .controls { display: flex; flex-wrap: wrap; align-items: center; gap: .75rem; padding: 1rem; border: 1px solid #394139; background: #121512; } .live { margin-left: auto; color: #aeb5ad; font-size: .85rem; }
  button { min-height: 2.5rem; padding: .55rem 1rem; border: 1px solid #525b52; background: #202420; color: inherit; font-weight: 800; cursor: pointer; } button:disabled { opacity: .5; cursor: not-allowed; } .danger { border-color: #ff8b8b; background: #291313; color: #ffb0b0; }
  a:focus-visible, button:focus-visible { outline: 3px solid #f4d35e; outline-offset: 3px; }
  main > section:not(.controls):not(.error):not(.lead) { padding: 0; border: 0; background: none; } #board-title { margin-bottom: .75rem; }
  .error, .notice, .warning, .state { margin: 0; padding: 1rem; border: 1px solid #697169; } .error { display: grid; gap: .4rem; border-color: #ff8b8b; color: #ffb0b0; } .notice { border-color: #65a978; } .warning { border-color: #d8c98a; color: #f2e6aa; }
  @media (max-width: 700px) { main { padding: 1rem; } .live { margin-left: 0; } }
</style>
