<script lang="ts">
  import { tick } from 'svelte';

  // Aksi berisiko/destruktif selalu lewat dua langkah: tombol "X…" membuka panel konfirmasi yang menjelaskan
  // akibatnya, meminta alasan bila perlu, lalu baru "Konfirmasi". Escape atau "Keep as is" membatalkan dan
  // mengembalikan fokus ke tombol pemicu.
  let {
    label,
    confirmLabel,
    tone = 'normal',
    description,
    reasonRequired = false,
    reasonLabel = 'Reason',
    disabled = false,
    disabledReason = '',
    busy = false,
    validate,
    onconfirm
  }: {
    label: string;
    confirmLabel: string;
    tone?: 'normal' | 'risky' | 'destructive';
    description: string;
    reasonRequired?: boolean;
    reasonLabel?: string;
    disabled?: boolean;
    disabledReason?: string;
    busy?: boolean;
    /** Validasi tambahan (mis. identitas pelaku); mengembalikan pesan masalah atau null. */
    validate?: (reason: string) => string | null;
    onconfirm: (reason: string) => void;
  } = $props();

  const uid = $props.id();
  let open = $state(false);
  let reason = $state('');
  let problem = $state('');
  let panel = $state<HTMLElement>();
  let trigger = $state<HTMLButtonElement>();

  $effect(() => {
    if (open) panel?.querySelector<HTMLElement>('textarea, button')?.focus();
  });

  async function close() {
    open = false;
    problem = '';
    await tick();
    trigger?.focus();
  }

  function confirm() {
    const message = validate?.(reason) ?? (reasonRequired && !reason.trim() ? `${reasonLabel} is required.` : null);
    if (message) {
      problem = message;
      return;
    }
    const value = reason.trim();
    reason = '';
    open = false;
    onconfirm(value);
  }
</script>

{#if !open}
  <span class="trigger">
    <button type="button" class={tone} bind:this={trigger} disabled={disabled || busy} aria-describedby={disabled && disabledReason ? `${uid}-why` : undefined} onclick={() => (open = true)}>{busy ? 'Working…' : `${label}…`}</button>
    {#if disabled && disabledReason}<span id={`${uid}-why`} class="why">{disabledReason}</span>{/if}
  </span>
{:else}
  <div class="panel {tone}" role="alertdialog" aria-modal="false" aria-labelledby={`${uid}-title`} aria-describedby={`${uid}-desc`} tabindex="-1" bind:this={panel} onkeydown={(event) => event.key === 'Escape' && close()}>
    <strong id={`${uid}-title`}>{label}?</strong>
    <p id={`${uid}-desc`}>{description}</p>
    {#if reasonRequired}
      <label>{reasonLabel}<textarea bind:value={reason} rows="2" aria-invalid={problem ? 'true' : undefined} aria-describedby={problem ? `${uid}-problem` : undefined}></textarea></label>
    {/if}
    {#if problem}<p id={`${uid}-problem`} class="problem" role="alert">{problem}</p>{/if}
    <div class="actions">
      <button type="button" class={tone} onclick={confirm}>{confirmLabel}</button>
      <button type="button" onclick={close}>Keep as is</button>
    </div>
  </div>
{/if}

<style>
  .trigger { display: inline-flex; flex-direction: column; gap: .25rem; } .why { color: #d8c98a; font-size: .8rem; }
  button { min-height: 2.5rem; padding: .5rem 1rem; border: 1px solid #525b52; background: #202420; color: inherit; font-weight: 800; cursor: pointer; } button:disabled { opacity: .5; cursor: not-allowed; }
  button.normal { background: #90e0a8; color: #071008; border: 0; } button.risky { border-color: #d8c98a; background: #292612; color: #f2e6aa; } button.destructive { border-color: #ff8b8b; background: #291313; color: #ffb0b0; }
  button:focus-visible, textarea:focus-visible { outline: 3px solid #fff; outline-offset: 3px; }
  .panel { display: grid; gap: .6rem; min-width: min(100%, 22rem); padding: .9rem; border: 2px solid #90e0a8; background: #0c0e0c; } .panel.risky { border-color: #d8c98a; } .panel.destructive { border-color: #ff8b8b; }
  .panel p { margin: 0; } label { display: grid; gap: .35rem; font-weight: 700; } textarea { width: 100%; box-sizing: border-box; padding: .5rem; border: 1px solid #525b52; background: #090b0a; color: inherit; font: inherit; }
  .problem { color: #ffb0b0; } .actions { display: flex; flex-wrap: wrap; gap: .6rem; }
</style>
