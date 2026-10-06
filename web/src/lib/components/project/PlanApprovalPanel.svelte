<script lang="ts">
  import type { PlanApprovalInput, ProposedPlan } from './types';

  let {
    plan,
    actorId = '',
    submitting = false,
    error = '',
    onsubmit
  }: { plan: ProposedPlan; actorId?: string; submitting?: boolean; error?: string; onsubmit?: (approval: PlanApprovalInput) => void } = $props();

  let actor = $state('');
  let reason = $state('');
  let reviewed = $state(false);
  let message = $state('');

  $effect(() => {
    actor = actorId;
  });

  const decidable = $derived(plan.status === 'PROPOSED' && !submitting);

  // Approve wajib konfirmasi eksplisit (manusia harus menyetujui plan sebelum eksekusi); reject wajib alasan.
  function decide(decision: 'APPROVED' | 'REJECTED') {
    if (!actor.trim()) return void (message = 'Actor ID is required.');
    if (decision === 'APPROVED' && !reviewed) return void (message = 'Confirm that you reviewed the tasks and risk flags.');
    if (decision === 'REJECTED' && !reason.trim()) return void (message = 'A reason is required to reject a plan.');
    message = '';
    onsubmit?.({ plan_id: plan.id, actor_id: actor.trim(), decision, reason: reason.trim() || null });
  }
</script>

<section aria-labelledby="approval-title">
  <h2 id="approval-title">Human approval</h2>
  {#if plan.status !== 'PROPOSED'}
    <p role="status">This plan is already {plan.status.toLowerCase()}; its decision can no longer change.</p>
  {:else}
    <p>Execution starts only after you approve this plan.</p>
  {/if}
  <fieldset disabled={!decidable}>
    <legend class="sr">Approval decision</legend>
    <label>Actor ID<input bind:value={actor} name="actor_id" autocomplete="off" required /></label>
    <label>Reason <span>(required when rejecting)</span><textarea bind:value={reason} name="reason" rows="3"></textarea></label>
    <label class="check"><input type="checkbox" bind:checked={reviewed} /> I reviewed {plan.tasks.length} tasks{plan.risk_flags.length ? ` and ${plan.risk_flags.length} risk flags` : ''}.</label>
    <div class="actions">
      <button type="button" class="approve" onclick={() => decide('APPROVED')}>{submitting ? 'Saving…' : 'Approve plan'}</button>
      <button type="button" class="reject" onclick={() => decide('REJECTED')}>Reject plan</button>
    </div>
  </fieldset>
  {#if message}<p class="error" role="alert">{message}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</section>

<style>
  section { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; } h2 { margin-top: 0; }
  fieldset { display: grid; gap: 1rem; margin: 0; padding: 0; border: 0; } .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
  label { display: grid; gap: .45rem; font-weight: 700; } label span { color: #aeb5ad; font-weight: 400; } .check { display: flex; align-items: center; gap: .6rem; } .check input { width: 1.25rem; min-height: 0; }
  input, textarea { width: 100%; box-sizing: border-box; min-height: 2.75rem; padding: .65rem .75rem; border: 1px solid #525b52; background: #090b0a; color: inherit; font: inherit; }
  input:focus-visible, textarea:focus-visible, button:focus-visible { outline: 3px solid #fff; outline-offset: 3px; }
  .actions { display: flex; flex-wrap: wrap; gap: .75rem; } button { min-height: 2.75rem; padding: .65rem 1rem; border: 0; font-weight: 800; cursor: pointer; } button:disabled { opacity: .6; }
  .approve { background: #90e0a8; color: #09100b; } .reject { background: #291313; color: #ffb0b0; border: 1px solid #ff8b8b; } .error { color: #ffb0b0; }
</style>
