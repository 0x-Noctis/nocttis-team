<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { asApiError, isCanonicalUuid, projectApi, type RepositoryMap } from '$lib/api/client';
  import type { ApiError } from '$lib/api/types';
  import { RunForm, RunStatusBadge, type ProjectInput, type ProjectRunInput, type ProjectRunView } from '$lib/components/project';

  let project = $state<ProjectInput | null>(null);
  let runs = $state<ProjectRunView[]>([]);
  let map = $state<RepositoryMap | null>(null);
  let loading = $state(true);
  let discovering = $state(false);
  let submitting = $state(false);
  let error = $state<ApiError | null>(null);
  let discoveryError = $state('');
  let formError = $state('');

  const projectId = $derived(page.params.id ?? '');
  const groups = $derived(map ? [
    ['Languages', map.languages], ['Frameworks', map.frameworks], ['Entry points', map.entry_points],
    ['Test commands', map.test_commands], ['Config files', map.config_files], ['Instruction files', map.instruction_files]
  ] as const : []);

  async function load() {
    loading = true;
    error = null;
    try { [project, runs] = await Promise.all([projectApi.get(projectId), projectApi.runs(projectId)]); }
    catch (reason) { error = asApiError(reason); }
    finally { loading = false; }
  }

  async function discover() {
    discovering = true;
    discoveryError = '';
    try { map = await projectApi.discover(projectId); }
    catch (reason) { discoveryError = asApiError(reason).error.message; }
    finally { discovering = false; }
  }

  async function createRun(input: ProjectRunInput) {
    if (!isCanonicalUuid(input.id)) {
      formError = `Run ID must be a lowercase UUID, for example ${crypto.randomUUID()}`;
      return;
    }
    submitting = true;
    formError = '';
    try {
      // project_id dipaksa sama dengan project halaman ini; server menolak (409) bila berbeda.
      const run = await projectApi.createRun(projectId, { ...input, project_id: projectId });
      await goto(`/runs/${encodeURIComponent(run.id)}`);
    } catch (reason) { formError = asApiError(reason).error.message; }
    finally { submitting = false; }
  }

  onMount(load);
</script>

<svelte:head><title>{project ? `${project.name} · Projects` : 'Project · Noctis'}</title></svelte:head>
<main>
  <nav aria-label="Breadcrumb"><a href="/projects">Projects</a><span aria-hidden="true">/</span><span>{project?.name ?? projectId}</span></nav>
  <h1>{project?.name ?? 'Project'}</h1>
  {#if error}<section class="error" role="alert"><strong>{error.error.message}</strong><span>Request ID: <code>{error.error.request_id}</code></span><button type="button" onclick={load}>Retry</button></section>{/if}
  {#if loading}<p class="state" aria-live="polite" aria-busy="true">Loading project…</p>
  {:else if project}
    <header><p class="eyebrow">Project</p><p><code>{project.repository_path}</code></p></header>

    <section aria-labelledby="discovery-title"><h2 id="discovery-title">Repository discovery</h2>
      <p>Maps files, languages, test commands, and instruction files without calling a model.</p>
      <button type="button" onclick={discover} disabled={discovering}>{discovering ? 'Discovering…' : map ? 'Run discovery again' : 'Run discovery'}</button>
      {#if discoveryError}<p class="bad" role="alert">{discoveryError}</p>{/if}
      {#if map}
        <p role="status">{map.files.length.toLocaleString()} files mapped{map.truncated ? ' (truncated: repository is larger than the scan limit)' : ''}.</p>
        <dl>{#each groups as [label, values] (label)}<div><dt>{label}</dt><dd>{#if values.length}{#each values as value}<code>{value}</code> {/each}{:else}None found{/if}</dd></div>{/each}</dl>
      {/if}
    </section>

    <section aria-labelledby="runs-title"><h2 id="runs-title">Runs</h2>
      {#if runs.length === 0}<p>No runs yet. Start one below.</p>
      {:else}<ul class="runs">{#each runs as run (run.id)}<li><a href={`/runs/${encodeURIComponent(run.id)}`}><strong>{run.objective}</strong><span>{run.id}</span></a><RunStatusBadge status={run.status} /></li>{/each}</ul>{/if}
    </section>

    <RunForm {projectId} {submitting} error={formError} onsubmit={createRun} />
  {/if}
</main>

<style>
  :global(*) { box-sizing: border-box; } :global(body) { margin: 0; background: #090b09; color: #eef2ec; font-family: system-ui, sans-serif; } main { display: grid; gap: 1.5rem; max-width: 76rem; margin: auto; padding: 2rem; }
  nav { display: flex; gap: .6rem; color: #aeb5ad; } a { color: #90e0a8; } h1 { margin: 0; font-size: clamp(2.2rem, 6vw, 4rem); } h2 { margin-top: 0; } .eyebrow { margin: 0; color: #90e0a8; font: 700 .8rem monospace; text-transform: uppercase; }
  section { padding: 1.25rem; border: 1px solid #394139; background: #121512; } dl { display: grid; gap: .7rem; } dt { color: #aeb5ad; } dd { margin: .15rem 0 0; overflow-wrap: anywhere; }
  .runs { display: grid; gap: .6rem; margin: 0; padding: 0; list-style: none; } .runs li { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 1rem; padding: .8rem; border: 1px solid #394139; } .runs a { display: grid; gap: .25rem; color: inherit; } .runs span { color: #aeb5ad; font-size: .8rem; }
  button { min-height: 2.5rem; padding: .55rem 1rem; border: 0; background: #90e0a8; color: #071008; font-weight: 800; cursor: pointer; } button:disabled { opacity: .55; cursor: wait; } a:focus-visible, button:focus-visible { outline: 3px solid #f4d35e; outline-offset: 3px; }
  .error, .state { margin: 0; padding: 1rem; border: 1px solid #697169; } .error { display: grid; gap: .4rem; border-color: #ff8b8b; color: #ffb0b0; } .error button { justify-self: start; background: #ffb0b0; color: #290909; } .bad { color: #ffb0b0; } @media (max-width: 700px) { main { padding: 1rem; } }
</style>
