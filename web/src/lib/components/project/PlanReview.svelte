<script lang="ts">
  import { isValidTaskLimits } from '$lib/api/types';
  import { reservedTokens, type ProposedPlan } from './types';

  // availableTokens = sisa budget run (limit - reserved); approval ditolak backend bila reservasi plan melebihinya.
  let { plan, availableTokens, loading = false, error = '' }: { plan?: ProposedPlan; availableTokens?: number; loading?: boolean; error?: string } = $props();

  const reserved = $derived(plan ? reservedTokens(plan.tasks) : null);
  const overBudget = $derived(reserved !== null && availableTokens !== undefined && reserved > availableTokens);
  const statusLabel = { PROPOSED: '? Proposed', APPROVED: '✓ Approved', REJECTED: '× Rejected' } as const;
</script>

{#if loading}
  <section class="state" aria-live="polite" aria-busy="true"><h2>Loading plan</h2><p>Waiting for the Lead planner…</p></section>
{:else if error}
  <section class="state error" role="alert"><h2>Plan unavailable</h2><p>{error}</p></section>
{:else if !plan}
  <section class="state"><h2>No plan yet</h2><p>The Lead planner has not proposed a plan for this run.</p></section>
{:else}
  <article aria-labelledby="plan-title">
    <header><div><p class="eyebrow">{plan.project_run_id} · version {plan.version}</p><h2 id="plan-title">Proposed plan · {plan.tasks.length} tasks</h2></div><strong class="status">{statusLabel[plan.status]}</strong></header>

    <section aria-labelledby="plan-risks"><h3 id="plan-risks">Risk flags</h3>
      {#if plan.risk_flags.length}<ul class="risks">{#each plan.risk_flags as flag}<li><span aria-hidden="true">⚠</span> {flag}</li>{/each}</ul>{:else}<p>No risk flags raised.</p>{/if}
    </section>

    <section aria-labelledby="plan-cost"><h3 id="plan-cost">Tokens reserved on approval</h3>
      {#if reserved === null}<p class="error" role="alert">Task limits produce an unsafe total.</p>
      {:else}<p>{reserved.toLocaleString()} <small>(Σ input + output per task)</small>{#if availableTokens !== undefined} of {availableTokens.toLocaleString()} remaining{/if}</p>
        {#if overBudget && plan.status === 'PROPOSED'}<p class="error" role="alert">Approval will be refused: this plan needs more tokens than the run has left.</p>{/if}
      {/if}
    </section>

    <h3>Tasks</h3>
    <ol class="tasks">
      {#each plan.tasks as task (task.id)}
        <li>
          <h4>{task.id} · {task.title}</h4>
          <p>{task.objective}</p>
          <dl>
            <div><dt>Role</dt><dd>{task.role}</dd></div>
            <div><dt>Depends on</dt><dd>{task.depends_on.length ? task.depends_on.join(', ') : 'None'}</dd></div>
            <div><dt>Allowed paths</dt><dd>{#each task.allowed_paths as path}<code>{path}</code> {/each}</dd></div>
            <div><dt>Verification</dt><dd>{#each task.verification_commands as command}<code>{command}</code> {/each}</dd></div>
            <div><dt>Limits</dt><dd>{#if isValidTaskLimits(task.limits)}{task.limits.max_input_tokens.toLocaleString()} in / {task.limits.max_output_tokens.toLocaleString()} out · {task.limits.max_attempts} attempts{:else}<span class="error">Invalid</span>{/if}</dd></div>
          </dl>
        </li>
      {/each}
    </ol>
  </article>
{/if}

<style>
  article, .state { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; } .error { color: #ffb0b0; } .state.error { border-color: #ff8b8b; }
  header { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 1rem; } h2 { margin: 0; font-size: 1.3rem; } .status { font: 700 .85rem ui-monospace, monospace; text-transform: uppercase; }
  .eyebrow { margin: 0 0 .35rem; color: #90e0a8; font: 700 .75rem/1.2 ui-monospace, monospace; letter-spacing: .12em; text-transform: uppercase; }
  .risks { padding-left: 1.2rem; color: #d8c98a; } small { color: #aeb5ad; }
  .tasks { display: grid; gap: .75rem; padding: 0; list-style: none; } .tasks li { padding: .85rem; border: 1px solid #394139; } h4 { margin: 0 0 .3rem; }
  dl { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: .5rem; margin: 0; } dt { color: #aeb5ad; font-size: .8rem; } dd { margin: .15rem 0 0; overflow-wrap: anywhere; }
  @media (max-width: 620px) { dl { grid-template-columns: 1fr; } }
</style>
