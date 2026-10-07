<script lang="ts">
  import { COMPONENT_LABELS, describeComponent, overallStatus } from './helpers';
  import type { ComponentName, OperationsView } from './types';

  let { view }: { view: Pick<OperationsView, 'ready' | 'components' | 'stale_attempts'> } = $props();

  const names = Object.keys(COMPONENT_LABELS) as ComponentName[];
  const overall = $derived(overallStatus(view));
</script>

<section aria-labelledby="health-title">
  <h2 id="health-title">Service health</h2>
  <p class="overall {view.ready ? (overall.icon === '✓' ? 'ok' : 'degraded') : 'down'}" role="status">
    <span aria-hidden="true">{overall.icon}</span> <strong>{overall.label}</strong> — {overall.detail}
  </p>
  <ul>
    {#each names as name (name)}
      {@const item = describeComponent(name, view.components[name], view)}
      <li class={item.state}>
        <h3>{item.title}</h3>
        <p class="state"><span aria-hidden="true">{item.icon}</span> {item.label}</p>
        <p class="why">{item.explanation}</p>
      </li>
    {/each}
  </ul>
</section>

<style>
  section { display: grid; gap: .75rem; padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  h2, h3, p { margin: 0; }
  .overall { padding: .6rem .8rem; border-left: 4px solid; }
  .overall.ok { border-color: #90e0a8; background: #102217; } .overall.degraded { border-color: #d8c98a; background: #292612; } .overall.down { border-color: #ff8b8b; background: #291313; }
  ul { display: grid; grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr)); gap: .75rem; margin: 0; padding: 0; list-style: none; }
  li { display: grid; gap: .35rem; padding: .85rem; border: 1px solid #394139; border-top: 4px solid #525b52; }
  li.ok { border-top-color: #90e0a8; } li.degraded { border-top-color: #d8c98a; } li.down { border-top-color: #ff8b8b; }
  .state { font-weight: 800; } .why { color: #aeb5ad; line-height: 1.4; }
</style>
