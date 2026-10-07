// Matriks skenario wajib E2E (M5-009). Setiap skenario diberi tag `[matrix:<id>]` di judul test; `matrix.spec.cjs`
// memastikan tidak ada tag wajib yang hilang. Dokumentasi: docs/test-matrix.md.
const REQUIRED = [
  // Skenario wajib rancangan (docs/RANCANGAN.md, "End-to-End Fixture")
  { id: 'parallel-independent', source: 'rancangan #1', what: 'dua task independen berjalan paralel' },
  { id: 'overlap-held', source: 'rancangan #2', what: 'dua task overlap ditahan scheduler' },
  { id: 'review-retry', source: 'rancangan #3', what: 'review meminta perubahan, attempt kedua lulus' },
  { id: 'provider-timeout-retry', source: 'rancangan #4', what: 'provider timeout memicu retry aman' },
  { id: 'restart-recovery', source: 'rancangan #5', what: 'server mati (SIGKILL) dan run pulih tanpa efek ganda' },
  { id: 'budget-stop', source: 'rancangan #6', what: 'budget habis menghentikan task' },
  { id: 'test-failure-blocks-integration', source: 'rancangan #7', what: 'test gagal mencegah integrasi' },
  // Tambahan dari task M5-009
  { id: 'backend-frontend-parallel', source: 'M5-009', what: 'Lead -> approval -> backend dan frontend paralel -> integrasi' },
  { id: 'semantic-conflict', source: 'M5-009', what: 'konflik semantik di cabang integrasi butuh manusia' },
  { id: 'approval-human-decision', source: 'M5-009', what: 'keputusan manusia (retry) lewat UI dengan identitas dan konfirmasi' },
  { id: 'plan-approval', source: 'M5-009', what: 'plan disetujui manusia sebelum task dibuat' },
  { id: 'provider-transient-retry', source: 'M5-009', what: 'gangguan sementara provider (503) diulang dengan backoff' },
  { id: 'provider-permanent-error-no-retry', source: 'M5-009', what: 'error permanen provider (401) tidak diulang' },
  { id: 'vertical-slice', source: 'M5-009', what: 'provider -> task -> hasil terminal lewat UI' },
  { id: 'parallel-four-workers', source: 'M4-010', what: 'empat worker mengisi empat slot' }
];

// Celah yang diketahui dan SENGAJA tidak diuji E2E (dicatat di docs/test-matrix.md).
const KNOWN_GAPS = [
  { id: 'provider-cross-provider-fallback', why: 'fallback lintas provider belum diaktifkan di produksi (butuh config eksplisit); diuji di tingkat unit: tests/model_fallback.rs' }
];

module.exports = { REQUIRED, KNOWN_GAPS };
