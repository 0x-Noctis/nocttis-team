// Polling dengan backoff untuk halaman run. Dipakai karena backend belum punya SSE level run
// (GET /runs/:id/events/stream belum ada); begitu ada, cukup ganti pemanggil ini.

export interface PollerOptions {
  intervalMs: number;
  // Batas atas jeda saat gagal beruntun; jeda berlipat dua tiap kegagalan.
  maxBackoffMs?: number;
  // Tab tersembunyi tidak perlu polling; tick dilewati sampai terlihat lagi.
  isVisible?: () => boolean;
  onError?: (reason: unknown, failures: number) => void;
  onRecover?: () => void;
}

export interface Poller {
  stop: () => void;
  // Jalankan tick sekarang (mis. saat tab kembali terlihat atau koneksi pulih) tanpa menumpuk.
  refresh: () => void;
}

// `tick` mengembalikan 'stop' bila tidak perlu polling lagi (mis. run sudah selesai).
// Tick berikutnya baru dijadwalkan setelah tick sebelumnya selesai, jadi request tidak pernah menumpuk.
export function startPoller(tick: () => Promise<void | 'stop'>, options: PollerOptions): Poller {
  const { intervalMs, maxBackoffMs = 30_000, isVisible = () => true } = options;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let running = false;
  let stopped = false;
  let failures = 0;

  const schedule = (delay: number) => {
    if (stopped) return;
    clearTimeout(timer);
    timer = setTimeout(run, delay);
  };

  async function run() {
    if (stopped || running) return;
    if (!isVisible()) return schedule(intervalMs);
    running = true;
    try {
      const result = await tick();
      if (stopped) return;
      if (failures > 0) options.onRecover?.();
      failures = 0;
      if (result === 'stop') return void (stopped = true);
      schedule(intervalMs);
    } catch (reason) {
      if (stopped) return;
      failures += 1;
      options.onError?.(reason, failures);
      schedule(Math.min(intervalMs * 2 ** failures, maxBackoffMs));
    } finally {
      running = false;
    }
  }

  schedule(intervalMs);
  return {
    stop: () => {
      stopped = true;
      clearTimeout(timer);
    },
    refresh: () => {
      if (!stopped && !running) void run();
    }
  };
}
