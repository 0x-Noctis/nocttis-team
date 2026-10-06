<script lang="ts">
  import { budgetLevel, type BudgetLevel } from '../project/types';
  import { gaugePercent, isValidGauge } from '../workers/helpers';
  import type { GaugeValue } from '../workers/types';

  let { gauge, compact = false }: { gauge: GaugeValue; compact?: boolean } = $props();

  // Teks selalu menyertakan level; warna hanya pelengkap.
  const labels: Record<BudgetLevel, string> = {
    ok: 'Within budget',
    warning: 'Warning (70%)',
    checkpoint: 'Checkpoint (85%)',
    exhausted: 'Stopped (100%)'
  };
  const valid = $derived(isValidGauge(gauge));
  const percent = $derived(valid ? gaugePercent(gauge) : 0);
  const level = $derived(budgetLevel(percent));
  const format = (value: number) => value.toLocaleString();
</script>

<div class="gauge" class:compact aria-label={gauge.label}>
  <div class="head"><strong>{gauge.label}</strong>{#if gauge.estimated}<span class="tag">Estimated</span>{/if}</div>
  {#if !valid}
    <p class="bad" role="alert">Invalid budget numbers.</p>
  {:else}
    <progress class={level} max="100" value={Math.min(100, percent)} aria-label={`${gauge.label} committed`}>{percent.toFixed(0)}%</progress>
    <p class="status {level}" role={level === 'ok' ? undefined : 'status'}>{Math.floor(percent)}% · {labels[level]}</p>
    {#if !compact}
      <dl>
        <div><dt>Used</dt><dd>{format(gauge.used)}</dd></div>
        <div><dt>Held</dt><dd>{format(gauge.held)}</dd></div>
        <div><dt>Limit</dt><dd>{format(gauge.limit)}</dd></div>
      </dl>
      {#if gauge.reserve > 0}<p class="note">{format(gauge.reserve)} tokens kept in reserve for recovery and integration.</p>{/if}
    {/if}
  {/if}
</div>

<style>
  .gauge { display: grid; gap: .4rem; color: #eef2ec; } .head { display: flex; justify-content: space-between; gap: .5rem; } .tag { padding: .1rem .4rem; border: 1px solid #aeb5ad; color: #aeb5ad; font: 700 .7rem ui-monospace, monospace; text-transform: uppercase; }
  progress { width: 100%; height: .9rem; accent-color: #90e0a8; } progress.warning { accent-color: #d8c98a; } progress.checkpoint, progress.exhausted { accent-color: #ff8b8b; }
  .status { margin: 0; font-weight: 700; font-size: .9rem; } .warning { color: #d8c98a; } .checkpoint, .exhausted, .bad { color: #ffb0b0; }
  dl { display: flex; gap: 1rem; margin: 0; } dl div { display: grid; } dt { color: #aeb5ad; font-size: .75rem; } dd { margin: 0; font-weight: 800; } .note { margin: 0; color: #aeb5ad; font-size: .8rem; }
  .compact progress { height: .6rem; }
</style>
