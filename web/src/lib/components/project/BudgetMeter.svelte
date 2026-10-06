<script lang="ts">
  import { isNonNegativeSafeInteger, isPositiveSafeInteger } from '$lib/api/types';
  import { budgetLevel, type BudgetLevel, type RunBudget } from './types';

  let { budget, loading = false, error = '' }: { budget?: RunBudget; loading?: boolean; error?: string } = $props();

  const labels: Record<BudgetLevel, string> = {
    ok: 'Within budget',
    warning: 'Warning: 70% of budget committed',
    checkpoint: 'Checkpoint: 85% of budget committed',
    exhausted: 'Budget exhausted: new work must stop'
  };
  const valid = (value: RunBudget) =>
    isPositiveSafeInteger(value.limit) && isNonNegativeSafeInteger(value.used) && isNonNegativeSafeInteger(value.reserved);
  const committed = $derived(budget ? budget.used + budget.reserved : 0);
  const percent = $derived(budget && valid(budget) ? Math.min(100, (committed / budget.limit) * 100) : 0);
  const level = $derived(budgetLevel(budget && valid(budget) ? (committed / budget.limit) * 100 : 0));
  const format = (value: number) => value.toLocaleString();
</script>

<section aria-labelledby="budget-title">
  <header><h2 id="budget-title">Token budget</h2>{#if budget}<strong>{budget.estimated ? 'Estimated' : 'Measured'}</strong>{/if}</header>
  {#if loading}<p aria-live="polite" aria-busy="true">Loading budget…</p>
  {:else if error}<p class="error" role="alert">Budget unavailable: {error}</p>
  {:else if !budget}<p>No budget set.</p>
  {:else if !valid(budget)}<p class="error" role="alert">Budget contains an invalid number.</p>
  {:else}
    <progress class={level} max="100" value={percent} aria-label="Committed budget">{percent.toFixed(0)}%</progress>
    <p class="status {level}" role={level === 'ok' ? undefined : 'status'}>{percent.toFixed(0)}% committed · {labels[level]}</p>
    <dl>
      <div><dt>Used</dt><dd>{format(budget.used)}</dd></div>
      <div><dt>Reserved</dt><dd>{format(budget.reserved)}</dd></div>
      <div><dt>Remaining</dt><dd>{format(Math.max(0, budget.limit - committed))}</dd></div>
      <div><dt>Limit</dt><dd>{format(budget.limit)}</dd></div>
    </dl>
  {/if}
</section>

<style>
  section { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  header { display: flex; justify-content: space-between; gap: 1rem; } h2 { margin-top: 0; } header strong { color: #90e0a8; }
  progress { width: 100%; height: 1rem; accent-color: #90e0a8; } progress.warning { accent-color: #d8c98a; } progress.checkpoint, progress.exhausted { accent-color: #ff8b8b; }
  .status { margin: .6rem 0; font-weight: 700; } .warning { color: #d8c98a; } .checkpoint, .exhausted { color: #ffb0b0; }
  dl { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: .75rem; margin-bottom: 0; } dl div { padding: .75rem; border: 1px solid #394139; } dt { color: #aeb5ad; } dd { margin: .3rem 0 0; font-size: 1.2rem; font-weight: 800; } .error { color: #ffb0b0; }
  @media (max-width: 620px) { dl { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
</style>
