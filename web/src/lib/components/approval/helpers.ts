// Logika murni UI approval; hanya `import type` supaya bisa dites langsung dengan node.
import type { ContextEvent, PendingPlan } from './types';

/** Penjelasan kode error attempt dalam bahasa yang dipahami operator; kode tak dikenal ditampilkan apa adanya. */
export function explainError(code: string | null): string {
  switch (code) {
    case null:
    case '':
      return 'No error was recorded for the last attempt.';
    case 'recovery.tool_in_progress':
      return 'A tool call was still running when the worker stopped. Its result is unknown, so it was not retried automatically.';
    case 'recovery.side_effect_ambiguous':
      return 'The worker stopped while integrating its patch. The patch may be partly applied, so a person must check it.';
    case 'recovery.worktree_unverified':
      return 'The worker’s working copy is missing or could not be verified, so its progress cannot be trusted.';
    case 'recovery.stale':
      return 'The worker stopped reporting and was recovered.';
    default:
      return `Last attempt ended with ${code}.`;
  }
}

/** Error API 409 = keadaan berubah sejak daftar dimuat (plan sudah diputuskan, versi task bergeser, dst). */
export function isStale(error: { error: { code: string } } | null | undefined): boolean {
  return error?.error.code === 'CONFLICT';
}

/** Validasi keputusan sebelum dikirim; mengembalikan pesan masalah atau null. */
export function validateDecision(actor: string, reason: string, reasonRequired: boolean): string | null {
  if (!actor.trim()) return 'Enter who you are acting as before deciding.';
  if (actor.trim().length > 128) return 'Actor name is too long (128 characters max).';
  if (reasonRequired && !reason.trim()) return 'A reason is required.';
  if (reason.trim().length > 500) return 'The reason is too long (500 characters max).';
  return null;
}

/** Hal-hal yang membuat persetujuan plan berisiko; dipakai untuk peringatan dan teks konfirmasi. */
export function planRisks(plan: PendingPlan): string[] {
  const risks = [...plan.risk_flags];
  if (plan.over_budget) {
    risks.push(`Needs ${plan.reserved_tokens.toLocaleString()} tokens but only ${plan.available_tokens.toLocaleString()} remain; the server will refuse approval.`);
  }
  return risks;
}

/** Satu baris ringkas untuk event konteks: siapa, apa, dan alasan bila ada. */
export function describeEvent(event: ContextEvent): string {
  const who = event.actor_id ? `${event.actor_id} (${event.actor})` : event.actor;
  const what = event.from_status && event.to_status ? `${event.event_type}: ${event.from_status} → ${event.to_status}` : event.event_type;
  const detail = [event.payload.reason, event.payload.disposition]
    .filter((value): value is string => typeof value === 'string' && value.length > 0)
    .join(' · ');
  return detail ? `${who} · ${what} — ${detail}` : `${who} · ${what}`;
}
