<script lang="ts">
  import type { DiffLine, TaskDiff } from '$lib/api/types';
  let { diff, loading = false, error = '' }: { diff: TaskDiff | null; loading?: boolean; error?: string } = $props();
  const marker = (line: DiffLine) => line.kind === 'addition' ? '+' : line.kind === 'deletion' ? '−' : ' ';
  const label = (line: DiffLine) => line.kind === 'addition' ? 'Added line' : line.kind === 'deletion' ? 'Removed line' : 'Context line';
</script>

<section aria-labelledby="diff-panel-title"><h2 id="diff-panel-title">Diff</h2>
  {#if loading}<p aria-live="polite" aria-busy="true">Loading diff…</p>
  {:else if error}<p class="error" role="alert">Diff unavailable: {error}</p>
  {:else if !diff || diff.lines.length === 0}<p>No source changes.</p>
  {:else}<div class="diff" role="textbox" aria-multiline="true" aria-readonly="true" tabindex="0" aria-label={`Diff for ${diff.file}`}><header><code>{diff.file}</code></header><ol>{#each diff.lines as line}<li class={line.kind} aria-label={`${label(line)} ${line.new_line ?? line.old_line ?? ''}`}><span class="kind">{label(line)}</span><span class="number">{line.old_line ?? ''}</span><span class="number">{line.new_line ?? ''}</span><span aria-hidden="true" class="marker">{marker(line)}</span><code>{line.content}</code></li>{/each}</ol></div>{/if}
</section>

<style>
  section { padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; } h2 { margin-top: 0; }
  .diff { overflow: auto; max-height: 26rem; border: 1px solid #525b52; background: #090b0a; } .diff:focus-visible { outline: 3px solid #fff; outline-offset: 3px; }
  header { position: sticky; top: 0; padding: .65rem; border-bottom: 1px solid #525b52; background: #161a17; } ol { min-width: max-content; margin: 0; padding: 0; list-style: none; }
  li { display: grid; grid-template-columns: 8rem 3rem 3rem 1.5rem 1fr; min-height: 1.7rem; } li > span, li > code { padding: .2rem .4rem; } .number { color: #919991; text-align: right; user-select: none; }
  .addition { background: #102217; } .deletion { background: #291313; } .kind { font-size: .8rem; font-weight: 700; } .marker { font-weight: 900; text-align: center; } .error { color: #ffb0b0; }
  @media (max-width: 620px) { li { grid-template-columns: 7rem 2.5rem 2.5rem 1.2rem 1fr; } }
  @media (prefers-reduced-motion: reduce) { .diff { scroll-behavior: auto; } }
</style>
