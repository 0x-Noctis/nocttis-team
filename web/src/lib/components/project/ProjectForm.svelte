<script lang="ts">
  import type { ProjectInput } from './types';

  let { submitting = false, error = '', onsubmit }: { submitting?: boolean; error?: string; onsubmit?: (project: ProjectInput) => void } = $props();

  let project = $state<ProjectInput>({ id: '', name: '', repository_path: '' });
  let fieldError = $state('');

  function submit(event: SubmitEvent) {
    event.preventDefault();
    const value = { id: project.id.trim(), name: project.name.trim(), repository_path: project.repository_path.trim() };
    if (!value.id || !value.name || !value.repository_path) {
      fieldError = 'Project ID, name, and repository path are required.';
      return;
    }
    fieldError = '';
    onsubmit?.(value);
  }
</script>

<form onsubmit={submit} aria-labelledby="project-form-title">
  <div>
    <p class="eyebrow">Project</p>
    <h2 id="project-form-title">Register repository</h2>
  </div>
  <div class="fields">
    <label>Project ID<input bind:value={project.id} name="id" autocomplete="off" required /></label>
    <label>Name<input bind:value={project.name} name="name" autocomplete="off" required /></label>
    <label class="wide">Repository path
      <input bind:value={project.repository_path} name="repository_path" placeholder="/home/me/work/repo" autocomplete="off" required />
      <span>Harus berupa direktori Git yang ada di server.</span>
    </label>
  </div>
  {#if fieldError}<p class="error" role="alert">{fieldError}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <button type="submit" disabled={submitting}>{submitting ? 'Saving project…' : 'Save project'}</button>
</form>

<style>
  form { display: grid; gap: 1.25rem; padding: 1.5rem; border: 1px solid #394139; background: #121512; color: #eef2ec; }
  .eyebrow { margin: 0 0 .35rem; color: #90e0a8; font: 700 .75rem/1.2 ui-monospace, monospace; letter-spacing: .12em; text-transform: uppercase; } h2 { margin: 0; font-size: 1.4rem; }
  .fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; } .wide { grid-column: 1 / -1; }
  label { display: grid; gap: .45rem; font-weight: 700; } label span { color: #aeb5ad; font-size: .8rem; font-weight: 400; } .error { margin: 0; color: #ffb0b0; }
  input { width: 100%; min-height: 2.75rem; padding: .65rem .75rem; border: 1px solid #525b52; background: #090b0a; color: inherit; font: inherit; }
  input:focus-visible, button:focus-visible { outline: 3px solid #fff; outline-offset: 3px; }
  button { justify-self: start; min-height: 2.75rem; padding: .65rem 1rem; border: 0; background: #90e0a8; color: #09100b; font-weight: 800; cursor: pointer; } button:disabled { opacity: .6; cursor: wait; }
  @media (max-width: 680px) { .fields { grid-template-columns: 1fr; } }
</style>
