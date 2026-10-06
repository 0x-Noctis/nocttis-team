<script lang="ts">
  type Probe = {
    status: string;
    model: string;
    latency_ms: number;
    content: string;
    usage?: {
      prompt_tokens: number;
      completion_tokens: number;
      total_tokens: number;
    };
  };

  let loading = false;
  let result: Probe | null = null;
  let error = '';

  async function probe() {
    loading = true;
    result = null;
    error = '';
    try {
      const response = await fetch('http://127.0.0.1:7410/api/v1/providers/primary/probe', {
        method: 'POST'
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? 'Probe gagal');
      result = body;
    } catch (reason) {
      error = reason instanceof Error ? reason.message : 'Probe gagal';
    } finally {
      loading = false;
    }
  }
</script>

<svelte:head><title>AI Team Control Plane</title></svelte:head>

<main>
  <p class="eyebrow">CONTROL PLANE / FOUNDATION</p>
  <h1>AI Team</h1>
  <p class="intro">Validasi koneksi OpenAI-compatible sebelum worker, scheduler, dan Lead Agent dibuat.</p>
  <a class="providers-link" href="/providers">Kelola providers</a>
  <a class="providers-link" href="/projects">Projects &amp; runs</a>
  <a class="providers-link" href="/tasks">Tasks</a>

  <section aria-labelledby="provider-title">
    <div>
      <span class="status">PRIMARY</span>
      <h2 id="provider-title">Model provider</h2>
      <p>Konfigurasi dibaca dari environment backend. API key tidak dikirim ke browser.</p>
    </div>
    <button onclick={probe} disabled={loading}>{loading ? 'Memeriksa…' : 'Jalankan probe'}</button>
  </section>

  {#if result}
    <aside class="success" aria-live="polite">
      <strong>{result.model}</strong>
      <span>{result.latency_ms} ms</span>
      <span>{result.usage?.total_tokens ?? 'usage tidak tersedia'} token</span>
      <code>{result.content}</code>
    </aside>
  {/if}

  {#if error}
    <aside class="error" role="alert">{error}</aside>
  {/if}

  <div class="grid">
    <article><span>01</span><h3>Provider probe</h3><p>Autentikasi, format respons, latency, dan usage.</p></article>
    <article><span>02</span><h3>Single worker</h3><p>Task contract, context terbatas, patch, dan verification.</p></article>
    <article><span>03</span><h3>Task DAG</h3><p>Lead planning, approval manusia, dependency, dan paralelisme.</p></article>
  </div>
</main>

<style>
  :global(*) { box-sizing: border-box; }
  :global(body) { margin: 0; background: #0b0d0c; color: #eef2ec; font-family: Inter, ui-sans-serif, system-ui, sans-serif; }
  main { width: min(1080px, calc(100% - 40px)); margin: 0 auto; padding: 72px 0; }
  .eyebrow, .status { color: #90e0a8; font: 700 12px/1.2 ui-monospace, monospace; letter-spacing: .14em; }
  h1 { margin: 8px 0 12px; font-size: clamp(64px, 12vw, 148px); line-height: .86; letter-spacing: -.07em; }
  .intro { max-width: 660px; color: #aeb5ad; font-size: 20px; line-height: 1.6; }
  section { margin-top: 72px; padding: 28px; border: 1px solid #303630; background: #121512; display: flex; align-items: center; justify-content: space-between; gap: 24px; }
  h2 { margin: 7px 0; font-size: 28px; } section p, article p { margin: 0; color: #929b92; line-height: 1.5; }
  button { border: 0; padding: 14px 20px; background: #90e0a8; color: #09100b; font-weight: 800; cursor: pointer; }
  button:disabled { opacity: .55; cursor: wait; }
  button:focus-visible { outline: 3px solid #fff; outline-offset: 3px; }
  aside { margin-top: 16px; padding: 18px 22px; display: flex; flex-wrap: wrap; gap: 18px; border-left: 4px solid; }
  .success { background: #102217; border-color: #90e0a8; } .error { background: #291313; border-color: #ff7474; }
  code { color: #90e0a8; }
  .providers-link { display: inline-block; margin-top: 18px; color: #90e0a8; font-weight: 800; }
  .providers-link + .providers-link { margin-left: 24px; }
  .providers-link:focus-visible { outline: 3px solid #fff; outline-offset: 3px; }
  .grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 1px; margin-top: 72px; background: #303630; border: 1px solid #303630; }
  article { min-height: 190px; padding: 26px; background: #0b0d0c; } article span { color: #687068; font-family: ui-monospace, monospace; } h3 { margin: 36px 0 8px; }
  @media (max-width: 700px) { main { padding-top: 40px; } section { align-items: stretch; flex-direction: column; } .grid { grid-template-columns: 1fr; } }
</style>
