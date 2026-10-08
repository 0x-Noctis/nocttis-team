# Catatan Rilis MVP — Release Candidate

**Status: DITAHAN.** Seluruh gate otomatis lulus, worker/reviewer berfungsi dengan model nyata, dan setelah optimasi token (M5-014)
**median input token turun 57,4%** dari baseline di dua run berturut-turut (target ≥ 50% tercapai). Namun gate "≥ 80% skenario tanpa
intervensi" tidak stabil: 4/5 (80%) di run 1 tetapi 3/5 (60%) di run 2, gabungan 7/10 = 70%. Karena itu rilis belum dapat
dinyatakan layak. Rincian dan opsi: [benchmark.md](benchmark.md).

Basis: cabang `agent-1/m5-013-real-model` di atas `main` lokal, dijalankan 2026-10-08.

## Hasil gate

| Gate | Command | Hasil |
|---|---|---|
| Format | `cargo fmt --check` | lulus |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | lulus |
| Unit + integrasi Rust | `cargo test --no-fail-fast -- --test-threads=1` dengan PostgreSQL 16 (lihat docs/development.md) | **450 lulus, 0 gagal**, 51 binary, 0 dilewati |
| Security suite | `tests/security.rs`, `tests/tool_policy.rs`, `tests/process_runner.rs`, `tests/context_builder.rs` (bagian dari baris di atas) | lulus |
| Backup/restore drill | `tests/backup` (database nyata) | 4 test lulus, benar-benar berjalan (bukan dilewati) |
| WebApp | `cd web && npm run check` dan `npm run build` | 0 error, build sukses |
| Compose | `PRIMARY_API_KEY=dummy docker compose config --quiet` | lulus (tanpa variable itu gagal; dokumen diperbaiki) |
| E2E matrix + audit rilis | `cd web && NOCTIS_E2E_API_KEY=… npm run e2e` | **60 lulus** (2,7 menit); sebelumnya 5× berturut-turut lulus pada M5-009 |
| Fresh install dari dokumen | README/operations.md di clone bersih (M5-011) | `health/ready` 200, UI dan API terlayani, container non-root read-only |
| Benchmark vs baseline (model nyata, setelah M5-014) | `node tests/baseline/mvp-run.cjs` lalu `compare.cjs` | **median token −57,4% di 2 run (lulus)**; tanpa intervensi 4/5 lalu 3/5 (gabungan 70%, **tidak stabil**) |

## Kriteria acceptance M5-012

| Kriteria | Status | Bukti |
|---|---|---|
| Median input token turun ≥ 50% dari baseline | **terpenuhi (57,4%, dua run)** | median 61.118 → 26.059 / 26.019 token (target ≤ 30.559); sebelum optimasi 47,0%; `tests/baseline/mvp-results*.json`; docs/benchmark.md |
| ≥ 80% task fixture selesai tanpa intervensi manual | **tidak stabil**: 80% lalu 60% (gabungan 70%) | `file-conflict` selalu gagal (acceptance tak dapat dipenuhi satu worker); `test-failure` gagal di run 2 (diff kosong ditolak reviewer, tanpa percobaan ulang). Satu run per skenario |
| Tidak ada edit di luar allowed paths | terpenuhi (diuji) | `patches_outside_the_allowed_paths_are_rejected_without_touching_git` (`tests/integrator.rs`), allowlist `ToolPolicy` (`tests/tool_policy.rs`, `tests/security.rs`) |
| Semua task `DONE` punya review, verification, patch, event, usage | terpenuhi (diuji) | `tests/e2e/zz-release-audit.spec.cjs`: 21 task `DONE` hasil orkestrasi, semuanya lengkap; 4 `DONE` lain adalah seeding fixture UI tanpa event dan dilaporkan terpisah. Audit gagal bila bukti dihapus (mutation check) |
| Restart tidak kehilangan status | terpenuhi (diuji) | E2E `[matrix:restart-recovery]` (SIGKILL lalu pulih), `tests/recovery.rs`, `tests/scheduler_parallel.rs` |
| 2–4 worker dapat dipantau dan dihentikan | terpenuhi (diuji) | E2E `[matrix:parallel-four-workers]` (dashboard), `cancel_stops_the_slot_and_finalizes_the_task`, `pause_cancel_and_budget_stop_before_claim`, `mutation_boundaries_and_paused_cancel` |
| Branch dasar aman pada semua skenario gagal | terpenuhi (diuji) | `baseIntact` pada skenario timeout, error permanen, test gagal, semantic conflict, restart di `tests/e2e/matrix.spec.cjs` dan `parallel.spec.cjs`; `tests/integrator.rs` |

## Yang menahan rilis

1. **Gate "tanpa intervensi" tidak andal** (70% gabungan dua run). Pilihan pemilik: (a) menetapkan eskalasi aman ke manusia untuk
   `file-conflict` (acceptance yang tak dapat dipenuhi satu worker) sebagai bukan kegagalan, dengan alasan tertulis; (b) menaikkan
   `max_attempts` bawaan Lead menjadi 2 agar penolakan reviewer memicu satu percobaan ulang (menambah token, perlu diukur ulang);
   (c) mengukur ≥ 3 run per skenario dan memakai persentase gabungan sebagai bukti.
2. **Putuskan blocker keamanan yang belum disepakati** (terdokumentasi, bukan cacat tersembunyi): API/UI tanpa autentikasi dan TLS
   (otentikasi ditunda atas permintaan pemilik; mitigasi: port hanya `127.0.0.1`), dan pemeriksaan anti-SSRF provider hanya pada probe.
3. **Push ke GitHub terblokir** oleh fixture token Slack palsu di `tests/security.rs:53` (commit `b24ed78`): gunakan URL unblock GitHub
   (tokennya palsu) atau tulis ulang riwayat lokal yang belum dipush.

Sudah teratasi (M5-013, M5-014): worker/reviewer tidak berfungsi dengan model nyata, status run tidak pernah `DONE`, dan gate token.

## Known limitations

Keamanan dan operasional (rincian: [security.md](security.md), [operations.md](operations.md)):
- Tidak ada autentikasi/otorisasi, TLS, dan rate limiting. `actor_id` pada approval tidak diverifikasi.
- Akses Docker untuk verifikasi (`compose.docker-socket.yaml`) setara root di host; tidak aktif secara bawaan, sehingga pada Compose
  bawaan task tidak dapat diverifikasi.
- Redaksi secret mengurangi kebocoran, tidak menjaminnya; artifact diff dan isi database tidak diredaksi.
- Arsip backup tidak dienkripsi; repository Git proyek dan cabang `noctis-integration-<run>` tidak ikut backup.
- CSP memakai `'unsafe-inline'` (batasan SvelteKit adapter-static).

Fungsional:
- Satu image verifikasi untuk seluruh server (`NOCTIS_RUNNER_IMAGE`, bawaan `rust:1`).
- Fallback ke model lain belum aktif di produksi (hanya retry pada model yang sama).
- Usage panggilan Lead belum dicatat di `model_usage` (benchmark memakai proxy penghitung).
- Plan Lead dengan scope tumpang tindih ditolak (skenario `file-conflict` benchmark gagal karenanya); pengguna harus meminta plan lagi.
- Konfigurasi `budgets.*`, `runner.network_enabled`, dan `server.data_dir` tidak berpengaruh (hanya ditampilkan/tidak dibaca).
- Provider di mesin host tidak terjangkau dari container Compose tanpa konfigurasi jaringan tambahan.
- Hasil kerja hanya berupa cabang Git lokal; tidak ada push/PR otomatis.
- Pengujian browser hanya Chromium pada viewport desktop.

Pengujian:
- Matriks E2E memakai provider palsu deterministik; perilaku model nyata hanya diukur oleh benchmark (satu run per skenario, 2026-10-08), dan fake provider terbukti pernah menyembunyikan blocker besar (skema tool kosong, tanpa prompt sistem).
- Eksekusi task lewat Docker-outside-of-Docker tidak dicakup matriks.
- `cargo audit`/`npm audit` belum otomatis di CI.

## Mengulang seluruh gate

Prasyarat dan persiapan PostgreSQL: [development.md](development.md). Ringkasnya:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
DATABASE_URL=postgres://ai_team:ai_team_dev@127.0.0.1:55432/ai_team cargo test --no-fail-fast -- --test-threads=1
(cd web && npm run check && npm run build)
PRIMARY_API_KEY=dummy docker compose config --quiet
# hapus container PostgreSQL di port 55432, lalu:
(cd web && NOCTIS_E2E_API_KEY=fixture-key npm run e2e)
node --test tests/baseline/*.test.cjs && node tests/baseline/verify.mjs
```
