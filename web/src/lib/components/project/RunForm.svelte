<script lang="ts">
  import { MAX_SAFE_INTEGER, isPositiveSafeInteger } from '$lib/api/types';
  import { parseCriteria, type ProjectRunInput } from './types';

  let {
    projectId = '',
    submitting = false,
    error = '',
    onsubmit
  }: { projectId?: string; submitting?: boolean; error?: string; onsubmit?: (run: ProjectRunInput) => void } = $props();

  // ID diisi UUID acak karena backend mewajibkan UUID kanonik; tetap bisa diubah bila perlu.
  let id = $state(crypto.randomUUID());
  let project = $state('');
  let objective = $state('');
  let criteria = $state('');
  let budget = $state(100000);
  let errors = $state<Record<string, string>>({});

  $effect(() => {
    project = projectId;
  });

  // Validasi sama dengan ProjectRunInput di backend: id/objective wajib, minimal satu criteria, budget bilangan bulat positif.
  function submit(event: SubmitEvent) {
    event.preventDefault();
    const acceptance_criteria = parseCriteria(criteria);
    const next: Record<string, string> = {};
    if (!id.trim()) next.id = 'Run ID is required.';
    if (!project.trim()) next.project_id = 'Project ID is required.';
    if (!objective.trim()) next.objective = 'Objective is required.';
    if (!acceptance_criteria.length) next.criteria = 'Add at least one acceptance criterion (one per line).';
    if (!isPositiveSafeInteger(budget)) next.budget = `Token budget must be a whole number from 1 to ${MAX_SAFE_INTEGER}.`;
    errors = next;
    if (Object.keys(next).length) return;
    onsubmit?.({ id: id.trim(), project_id: project.trim(), objective: objective.trim(), acceptance_criteria, token_budget: budget });
  }
</script>

<form onsubmit={submit} aria-labelledby="run-form-title" novalidate>
  <div>
    <p class="eyebrow">Project run</p>
    <h2 id="run-form-title">Start a run</h2>
  </div>
  <div class="fields">
    <label>Run ID
      <input bind:value={id} name="id" autocomplete="off" aria-invalid={errors.id ? 'true' : undefined} aria-describedby={errors.id ? 'run-id-error' : 'run-id-hint'} required />
      <span id="run-id-hint">UUID dibuat otomatis.</span>
      {#if errors.id}<span id="run-id-error" class="error" role="alert">{errors.id}</span>{/if}
    </label>
    <label>Project ID
      <input bind:value={project} name="project_id" autocomplete="off" aria-invalid={errors.project_id ? 'true' : undefined} aria-describedby={errors.project_id ? 'run-project-error' : undefined} required />
      {#if errors.project_id}<span id="run-project-error" class="error" role="alert">{errors.project_id}</span>{/if}
    </label>
    <label class="wide">Objective
      <textarea bind:value={objective} name="objective" rows="3" aria-invalid={errors.objective ? 'true' : undefined} aria-describedby={errors.objective ? 'run-objective-error' : undefined} required></textarea>
      {#if errors.objective}<span id="run-objective-error" class="error" role="alert">{errors.objective}</span>{/if}
    </label>
    <label class="wide">Acceptance criteria
      <textarea bind:value={criteria} name="acceptance_criteria" rows="4" aria-invalid={errors.criteria ? 'true' : undefined} aria-describedby={errors.criteria ? 'run-criteria-error' : 'run-criteria-hint'} required></textarea>
      <span id="run-criteria-hint">Satu kriteria per baris.</span>
      {#if errors.criteria}<span id="run-criteria-error" class="error" role="alert">{errors.criteria}</span>{/if}
    </label>
    <label>Token budget
      <input bind:value={budget} name="token_budget" type="number" min="1" max={MAX_SAFE_INTEGER} step="1" aria-invalid={errors.budget ? 'true' : undefined} aria-describedby={errors.budget ? 'run-budget-error' : undefined} required />
      {#if errors.budget}<span id="run-budget-error" class="error" role="alert">{errors.budget}</span>{/if}
    </label>
  </div>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <button type="submit" disabled={submitting}>{submitting ? 'Creating run…' : 'Create run'}</button>
</form>

<style>
  form { display: grid; gap: 1.25rem; padding: 1.5rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  .eyebrow { margin: 0 0 .35rem; color: #90e0a8; font: 700 .75rem/1.2 ui-monospace, monospace; letter-spacing: .12em; text-transform: uppercase; } h2 { margin: 0; font-size: 1.4rem; }
  .fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; } .wide { grid-column: 1 / -1; }
  label { display: grid; gap: .45rem; font-weight: 700; } label span { color: #aeb5ad; font-size: .8rem; font-weight: 400; } .error { margin: 0; color: #ffb0b0; } label .error { color: #ffb0b0; }
  input, textarea { width: 100%; box-sizing: border-box; min-height: 2.75rem; padding: .65rem .75rem; border: 1px solid #525b52; background: #090b0a; color: inherit; font: inherit; }
  input:focus-visible, textarea:focus-visible, button:focus-visible { outline: 3px solid #fff; outline-offset: 3px; }
  button { justify-self: start; min-height: 2.75rem; padding: .65rem 1rem; border: 0; background: #90e0a8; color: #09100b; font-weight: 800; cursor: pointer; } button:disabled { opacity: .6; cursor: wait; }
  @media (max-width: 680px) { .fields { grid-template-columns: 1fr; } }
</style>
