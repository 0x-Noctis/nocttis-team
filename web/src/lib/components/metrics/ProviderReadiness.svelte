<script lang="ts">
  import { providerIssues } from './helpers';
  import type { ProviderReadinessView } from './types';

  let { providers }: { providers: ProviderReadinessView[] } = $props();

  const toolsLabel = { supported: 'Tools verified', unsupported: 'Tools unsupported', unknown: 'Tools not verified' } as const;
</script>

<section aria-labelledby="providers-title">
  <h2 id="providers-title">Provider readiness</h2>
  {#if providers.length === 0}
    <p>No providers registered. <a href="/providers">Add a provider</a> to run tasks.</p>
  {:else}
    <ul>
      {#each providers as provider (provider.id)}
        {@const issues = providerIssues(provider)}
        <li class={issues.length ? 'attention' : 'ready'}>
          <h3>{provider.id} <span class="host">{provider.host}</span></h3>
          <p class="status"><span aria-hidden="true">{issues.length ? '!' : '✓'}</span> {issues.length ? 'Needs attention' : 'Ready'}</p>
          <p>API key: <strong>{provider.secret_configured ? 'configured on server' : 'missing'}</strong></p>
          {#if provider.models.length}
            <ul class="models">{#each provider.models as model (model.id)}<li><code>{model.id}</code>{#if model.class} · {model.class}{/if} · {toolsLabel[model.tools]}</li>{/each}</ul>
          {/if}
          {#if issues.length}<ul class="issues">{#each issues as issue (issue)}<li>{issue}</li>{/each}</ul>{/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  section { display: grid; gap: .75rem; padding: 1.25rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  h2, h3, p { margin: 0; } a { color: #90e0a8; }
  section > ul { display: grid; grid-template-columns: repeat(auto-fit, minmax(18rem, 1fr)); gap: .75rem; margin: 0; padding: 0; list-style: none; }
  section > ul > li { display: grid; gap: .4rem; padding: .85rem; border: 1px solid #394139; border-top: 4px solid #90e0a8; } li.attention { border-top-color: #d8c98a; }
  .host { color: #aeb5ad; font: 400 .85rem ui-monospace, monospace; } .status { font-weight: 800; }
  .models, .issues { margin: 0; padding-left: 1.1rem; } .issues { color: #f2e6aa; } code { color: #90e0a8; }
</style>
