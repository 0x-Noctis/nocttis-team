// Logika murni dashboard paralel; tanpa import runtime supaya bisa dites langsung dengan node.
import type { GaugeValue } from './types';

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
