<script lang="ts">
  import { formatConfigValue, humanize } from './helpers';
  import type { ConfigView } from './types';

  let { config }: { config: ConfigView } = $props();
</script>

<div class="groups">
  {#each Object.entries(config) as [group, values] (group)}
    <section aria-labelledby={`config-${group}`}>
      <h2 id={`config-${group}`}>{humanize(group)}</h2>
      <dl>
        {#each Object.entries(values as Record<string, unknown>) as [key, value] (key)}
          <div><dt>{humanize(key)}</dt><dd>{formatConfigValue(value)}</dd></div>
        {/each}
      </dl>
    </section>
  {/each}
</div>

<style>
  .groups { display: grid; grid-template-columns: repeat(auto-fit, minmax(20rem, 1fr)); gap: 1rem; }
  section { display: grid; gap: .6rem; padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; align-content: start; }
  h2 { margin: 0; font-size: 1.15rem; } dl { display: grid; gap: .3rem; margin: 0; }
  dl div { display: grid; grid-template-columns: minmax(8rem, 1fr) 1.4fr; gap: .75rem; padding: .3rem 0; border-bottom: 1px solid #262c26; }
  dt { color: #aeb5ad; } dd { margin: 0; overflow-wrap: anywhere; font-variant-numeric: tabular-nums; }
</style>
