<script lang="ts">
  import { capabilityKeys, type ModelCapabilities, type VerifiedCapability } from '$lib/api/types';

  let { capabilities }: { capabilities: ModelCapabilities } = $props();

  const labels = {
    chat: 'Chat',
    streaming: 'Streaming',
    tools: 'Tools',
    parallel_tools: 'Parallel tools'
  } as const;

  const verifiedLabels: Record<VerifiedCapability, string> = {
    unknown: 'Unknown — not verified',
    supported: 'Supported — probe passed',
    unsupported: 'Unsupported — probe failed'
  };
</script>

<div class="table-wrap" role="region" aria-label="Model capability matrix">
  <table>
    <thead><tr><th scope="col">Capability</th><th scope="col">Claimed</th><th scope="col">Verified</th></tr></thead>
    <tbody>
      {#each capabilityKeys as capability}
        <tr>
          <th scope="row">{labels[capability]}</th>
          <td>{capabilities.claimed[capability] ? 'Yes — provider claim' : 'No — not claimed'}</td>
          <td><span class:unknown={capabilities.verified[capability] === 'unknown'}>{verifiedLabels[capabilities.verified[capability]]}</span></td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .table-wrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; font-size: .9rem; }
  th, td { padding: .7rem; border-bottom: 1px solid #394139; text-align: left; }
  thead th { color: #aeb5ad; font-size: .75rem; letter-spacing: .08em; text-transform: uppercase; }
  tbody th { font-weight: 700; }
  .unknown { color: #d8c98a; }
</style>
