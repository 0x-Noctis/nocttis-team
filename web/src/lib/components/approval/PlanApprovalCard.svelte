<script lang="ts">
  import ConfirmAction from './ConfirmAction.svelte';
  import { planRisks, validateDecision } from './helpers';
  import type { PendingPlan } from './types';

  let {
    plan,
    actor,
    busy = false,
    error = '',
    onapprove,
    onreject,
    onrefresh
  }: {
    plan: PendingPlan;
    actor: string;
    busy?: boolean;
    error?: string;
    onapprove: (reason: string) => void;
    onreject: (reason: string) => void;
    onrefresh?: () => void;
  } = $props();

  const risks = $derived(planRisks(plan));
  const actorProblem = $derived(validateDecision(actor, '', false));
  const approveDescription = $derived(
    `Approving reserves ${plan.reserved_tokens.toLocaleString()} tokens and lets ${plan.task_count} task${plan.task_count === 1 ? '' : 's'} start. ` +
      (plan.risk_flags.length ? `Risk flags raised: ${plan.risk_flags.join('; ')}. ` : 'No risk flags were raised. ') +
      `This is recorded as ${actor.trim() || '(no actor)'}.`
  );
</script>

<article aria-labelledby={`plan-${plan.plan_id}`}>
  <header>
    <div><p class="eyebrow">{plan.project_name} · run {plan.run_status.toLowerCase().replace('_', ' ')}</p><h3 id={`plan-${plan.plan_id}`}>{plan.run_objective}</h3></div>
    <strong class="version">Plan v{plan.version}</strong>
  </header>
  <dl>
    <div><dt>Tasks</dt><dd>{plan.task_count}</dd></div>
    <div><dt>Tokens to reserve</dt><dd>{plan.reserved_tokens.toLocaleString()}</dd></div>
    <div><dt>Budget left</dt><dd>{plan.available_tokens.toLocaleString()}</dd></div>
    <div><dt>Proposed</dt><dd><time datetime={plan.created_at}>{plan.created_at.replace('T', ' ').replace('Z', ' UTC')}</time></dd></div>
  </dl>
  {#if risks.length}
    <div class="risks" role="note"><strong>Risks</strong><ul>{#each risks as risk}<li><span aria-hidden="true">⚠</span> {risk}</li>{/each}</ul></div>
  {/if}
  <p><a href={`/runs/${encodeURIComponent(plan.run_id)}`}>Review the full plan and DAG on the run page</a></p>
  <div class="actions">
    <ConfirmAction label="Approve plan" confirmLabel="Yes, approve" tone="risky" description={approveDescription} {busy}
      disabled={plan.over_budget || !!actorProblem} disabledReason={plan.over_budget ? 'Not enough budget; reject it or raise the run budget.' : (actorProblem ?? '')}
      validate={() => validateDecision(actor, '', false)} onconfirm={onapprove} />
    <ConfirmAction label="Reject plan" confirmLabel="Yes, reject" tone="destructive" reasonRequired reasonLabel="Reason for rejecting" {busy}
      description={`Rejecting discards this proposal; the Lead can propose a new version. This is recorded as ${actor.trim() || '(no actor)'}.`}
      disabled={!!actorProblem} disabledReason={actorProblem ?? ''} validate={(reason) => validateDecision(actor, reason, true)} onconfirm={onreject} />
  </div>
  {#if error}<p class="error" role="alert">{error}{#if onrefresh} <button type="button" class="link" onclick={onrefresh}>Refresh the queue</button>{/if}</p>{/if}
</article>

<style>
  article { display: grid; gap: .75rem; padding: 1.1rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  header { display: flex; flex-wrap: wrap; justify-content: space-between; gap: .75rem; } h3 { margin: 0; font-size: 1.15rem; } .eyebrow { margin: 0 0 .25rem; color: #90e0a8; font: 700 .75rem ui-monospace, monospace; text-transform: uppercase; } .version { font: 700 .85rem ui-monospace, monospace; }
  dl { display: grid; grid-template-columns: repeat(auto-fit, minmax(9rem, 1fr)); gap: .5rem; margin: 0; } dl div { padding: .5rem; border: 1px solid #394139; } dt { color: #aeb5ad; font-size: .75rem; } dd { margin: .15rem 0 0; font-weight: 800; }
  .risks { padding: .6rem .8rem; border: 1px solid #d8c98a; color: #f2e6aa; } .risks ul { margin: .3rem 0 0; padding-left: 1.1rem; } a { color: #90e0a8; }
  .actions { display: flex; flex-wrap: wrap; gap: .75rem; align-items: start; } .error { margin: 0; color: #ffb0b0; } .link { padding: 0; border: 0; background: none; color: #90e0a8; text-decoration: underline; cursor: pointer; font: inherit; }
  a:focus-visible, .link:focus-visible { outline: 3px solid #f4d35e; outline-offset: 3px; }
</style>
