<script lang="ts">
  import type { RetentionView } from './types';

  let { retention }: { retention: RetentionView } = $props();

  const outcome = {
    deleted: { icon: '✓', label: 'Deleted' },
    would_delete: { icon: '○', label: 'Would delete (dry run)' },
    failed: { icon: '×', label: 'Failed' }
  } as const;
  const kind = { orphan_artifact: 'Orphan artifact', integration_worktree: 'Integration worktree' } as const;
</script>

<section aria-labelledby="retention-title">
  <h2 id="retention-title">Retention</h2>
  <p>Last cleanup: <strong>{retention.last_run_at ?? 'never'}</strong>. Last 24 hours: <strong>{retention.last_24h.deleted}</strong> deleted, <strong class:bad={retention.last_24h.failed > 0}>{retention.last_24h.failed}</strong> failed.
    {#if retention.last_24h.failed > 0}<span class="bad-note"><span aria-hidden="true">×</span> Failed cleanups are retried automatically on the next run.</span>{/if}</p>
  {#if retention.recent.length === 0}
    <p class="muted">No cleanup actions recorded yet.</p>
  {:else}
    <!-- Wilayah yang bisa digulir horizontal harus bisa difokus keyboard supaya isinya terjangkau (WCAG 2.1.1). -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div class="scroll" role="region" aria-label="Recent retention actions" tabindex="0">
      <table>
        <caption class="visually-hidden">Most recent retention actions</caption>
        <thead><tr><th scope="col">Time (UTC)</th><th scope="col">Kind</th><th scope="col">Target</th><th scope="col">Outcome</th></tr></thead>
        <tbody>
          {#each retention.recent as action, index (index)}
            <tr><td>{action.at}</td><td>{kind[action.kind]}</td><td><code>{action.target}</code></td>
              <td><span aria-hidden="true">{outcome[action.outcome].icon}</span> {outcome[action.outcome].label}{#if action.detail} — {action.detail}{/if}</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

<style>
  section { display: grid; gap: .75rem; padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  h2, p { margin: 0; } .muted { color: #aeb5ad; } .bad, .bad-note { color: #ffb0b0; }
  .scroll { overflow-x: auto; } table { width: 100%; border-collapse: collapse; min-width: 34rem; } th, td { padding: .45rem .6rem; border-bottom: 1px solid #262c26; text-align: left; vertical-align: top; }
  th { color: #aeb5ad; font-weight: 700; } code { color: #90e0a8; overflow-wrap: anywhere; }
  .visually-hidden { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
</style>
