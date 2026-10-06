<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { asApiError, isCanonicalUuid, projectApi } from '$lib/api/client';
  import type { ApiError } from '$lib/api/types';
  import { ProjectForm, type ProjectInput } from '$lib/components/project';

  let projects = $state<ProjectInput[]>([]);
  let loading = $state(true);
  let submitting = $state(false);
  let error = $state<ApiError | null>(null);
  let formError = $state('');

  async function load() {
    loading = true;
    error = null;
    try { projects = await projectApi.list(); }
    catch (reason) { error = asApiError(reason); }
    finally { loading = false; }
  }

  async function create(input: ProjectInput) {
    // ID project harus UUID kanonik; kalau bukan, beri contoh yang bisa langsung ditempel.
    if (!isCanonicalUuid(input.id)) {
      formError = `Project ID must be a lowercase UUID, for example ${crypto.randomUUID()}`;
      return;
    }
    submitting = true;
    formError = '';
    try {
      const created = await projectApi.create(input);
      await goto(`/projects/${encodeURIComponent(created.id)}`);
    } catch (reason) { formError = asApiError(reason).error.message; }
    finally { submitting = false; }
  }

  onMount(load);
</script>

<svelte:head><title>Projects · Noctis</title></svelte:head>
<main>
  <nav aria-label="Primary"><a href="/projects" aria-current="page">Projects</a><a href="/tasks">Tasks</a><a href="/providers">Providers</a></nav>
  <header><p class="eyebrow">Control plane</p><h1>Projects</h1><p>Register a Git repository, then start a run for the Lead planner.</p></header>
  {#if error}<section class="error" role="alert"><strong>{error.error.message}</strong><span>Request ID: <code>{error.error.request_id}</code></span><button type="button" onclick={load}>Retry</button></section>{/if}
  {#if loading}<p class="state" aria-live="polite" aria-busy="true">Loading projects…</p>
  {:else if !error && projects.length === 0}<p class="state">No projects yet. Register the first repository below.</p>
  {:else}<ul class="projects">{#each projects as project (project.id)}<li><a href={`/projects/${encodeURIComponent(project.id)}`}><strong>{project.name}</strong><code>{project.repository_path}</code><span>{project.id}</span></a></li>{/each}</ul>{/if}
  <ProjectForm {submitting} error={formError} onsubmit={create} />
</main>

<style>
  :global(*) { box-sizing: border-box; } :global(body) { margin: 0; background: #090b09; color: #eef2ec; font-family: system-ui, sans-serif; } main { display: grid; gap: 1.5rem; max-width: 76rem; margin: auto; padding: 2rem; }
  nav { display: flex; gap: 1.2rem; } a { color: #90e0a8; } a[aria-current='page'] { color: #fff; font-weight: 800; } h1 { margin: 0; font-size: clamp(2.5rem, 7vw, 5rem); } .eyebrow { margin: 0; color: #90e0a8; font: 700 .8rem monospace; text-transform: uppercase; }
  .projects { display: grid; grid-template-columns: repeat(auto-fit, minmax(18rem, 1fr)); gap: 1rem; margin: 0; padding: 0; list-style: none; } .projects a { display: grid; gap: .5rem; min-height: 7rem; padding: 1.2rem; border: 1px solid #394139; background: #121512; color: inherit; text-decoration: none; } .projects a:hover { border-color: #90e0a8; } .projects span { color: #aeb5ad; font-size: .8rem; overflow-wrap: anywhere; }
  .error, .state { margin: 0; padding: 1rem; border: 1px solid #697169; } .error { display: grid; gap: .4rem; border-color: #ff8b8b; color: #ffb0b0; } .error button { justify-self: start; padding: .5rem .9rem; border: 0; background: #ffb0b0; color: #290909; font-weight: 800; cursor: pointer; }
  a:focus-visible, button:focus-visible { outline: 3px solid #f4d35e; outline-offset: 3px; } @media (max-width: 700px) { main { padding: 1rem; } }
</style>
