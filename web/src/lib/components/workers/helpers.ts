// Logika murni dashboard paralel; tanpa import runtime supaya bisa dites langsung dengan node.
import type { GaugeValue, WorkerSlot } from './types';

/** Heartbeat: 'slow' mulai separuh batas stale supaya operator melihat masalah sebelum claim diambil alih. */
export function heartbeatState(ageSeconds: number | undefined, staleAfterSeconds = 60): 'none' | 'fresh' | 'slow' | 'stale' {
  if (ageSeconds === undefined || !Number.isFinite(ageSeconds) || ageSeconds < 0) return 'none';
  if (ageSeconds >= staleAfterSeconds) return 'stale';
  return ageSeconds >= staleAfterSeconds / 2 ? 'slow' : 'fresh';
}

/** Persentase pemakaian terhadap batas untuk pekerjaan biasa (limit - reserve); 0..100+ (tidak dipotong). */
export function gaugePercent(gauge: GaugeValue): number {
  const workLimit = gauge.limit - gauge.reserve;
  if (!Number.isFinite(workLimit) || workLimit <= 0) return 100;
  return ((gauge.used + gauge.held) / workLimit) * 100;
}

export function isValidGauge(gauge: GaugeValue): boolean {
  return [gauge.limit, gauge.used, gauge.held, gauge.reserve].every((value) => Number.isSafeInteger(value) && value >= 0) && gauge.limit > 0 && gauge.reserve < gauge.limit;
}

/** Buang duplikat berkunci sama (yang pertama menang) supaya `{#each}` berkunci tidak melempar error. */
export function uniqueBy<T>(items: T[], key: (item: T) => string): T[] {
  const seen = new Set<string>();
  return items.filter((item) => {
    const id = key(item);
    if (seen.has(id)) return false;
    seen.add(id);
    return true;
  });
}

/**
 * Slot dari API hanya memuat slot aktif. Lengkapi sampai `maxSlots` dengan slot kosong supaya kapasitas terlihat;
 * pada run yang di-pause slot kosong berstatus `paused` (tidak akan diisi). Slot ganda (nomor sama) dibuang,
 * dan bila aktif > maxSlots semuanya tetap ditampilkan.
 */
export function padSlots(active: WorkerSlot[], maxSlots: number, runStatus: string): WorkerSlot[] {
  const slots = uniqueBy([...active].sort((a, b) => a.slot - b.slot), (slot) => String(slot.slot));
  const empty: WorkerSlot['state'] = runStatus === 'PAUSED' ? 'paused' : 'idle';
  const taken = new Set(slots.map((slot) => slot.slot));
  for (let number = 1; slots.length < Math.max(maxSlots, 0); number += 1) {
    if (!taken.has(number)) slots.push({ slot: number, state: empty, leases: [] });
  }
  return slots.sort((a, b) => a.slot - b.slot);
}

/** Kalimat status kontrol run: hasil pause/cancel yang masih berjalan terlihat sampai slot benar-benar kosong. */
export function controlNotice(runStatus: string, activeSlots: number): string {
  const running = `${activeSlots} running task${activeSlots === 1 ? ' is' : 's are'}`;
  if (runStatus === 'PAUSED') {
    return activeSlots > 0 ? `Run paused. ${running} finishing; nothing new will start.` : 'Run paused. No tasks are running.';
  }
  if (runStatus === 'CANCELLED') {
    return activeSlots > 0 ? `Run cancelled. Stopping ${activeSlots} worker${activeSlots === 1 ? '' : 's'}…` : 'Run cancelled. All workers have stopped.';
  }
  return '';
}
