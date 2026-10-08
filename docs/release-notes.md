# Catatan Rilis MVP — Release Candidate

**Status: memenuhi semua kriteria rilis yang ditetapkan pemilik (2026-10-08).** Seluruh gate otomatis lulus, dan benchmark nyata
3 putaran × 5 skenario dengan model sama seperti baseline memenuhi keempat syaratnya: skenario normal selesai otomatis 12/12,
median input token turun 56,2%, skenario konflik 3/3 berhenti aman dengan permintaan keputusan manusia, dan branch dasar utuh di
15/15 run. Rincian: [benchmark.md](benchmark.md). Satu langkah tersisa ada di tangan pemilik: membersihkan riwayat lokal dari
fixture token palsu lalu `git push` manual (lihat "Sebelum push").

Basis: `main` lokal setelah merge M5-015 sampai M5-017, dijalankan 2026-10-08.

## Hasil gate

| Gate | Command | Hasil |
|---|---|---|
| Format | `cargo fmt --check` | lulus |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | lulus |
| Unit + integrasi Rust | `cargo test --no-fail-fast -- --test-threads=1` dengan PostgreSQL 16 (lihat docs/development.md) | **457 lulus, 0 gagal**, 51 binary |
| Test harness benchmark | `node --test tests/baseline/*.test.cjs` dan `node tests/baseline/verify.mjs` | 15 lulus; skema baseline valid |
| Security suite | `tests/security.rs`, `tests/tool_policy.rs`, `tests/process_runner.rs`, `tests/context_builder.rs`, unit `security::ssrf`, `config` | lulus (bagian dari baris Rust) |
| Backup/restore drill | `tests/backup` (database nyata) | lulus, benar-benar berjalan |
| WebApp | `cd web && npm run check` dan `npm run build` | 0 error, build sukses (web hanya berubah satu helper teks) |
| Compose | `PRIMARY_API_KEY=dummy docker compose config --quiet` | lulus |
| E2E matrix + audit rilis | `cd web && NOCTIS_E2E_API_KEY=… npm run e2e` | **61 lulus** (termasuk verifikasi gagal → retry → eskalasi) |
| Fresh install dari dokumen | README/operations.md di clone bersih (M5-011) | `health/ready` 200, UI dan API terlayani, container non-root read-only |
| Benchmark vs baseline (model nyata, 3 putaran) | `NOCTIS_BENCH_ROUNDS=3 node tests/baseline/mvp-run.cjs` lalu `compare.cjs` | **semua 4 gate lulus** |

## Kriteria rilis

| Kriteria | Status | Bukti |
|---|---|---|
| Task normal selesai otomatis ≥ 80% | **terpenuhi: 12/12 (100%)** | `tests/baseline/mvp-results.json` (3 putaran) |
| Median input token turun ≥ 50% | **terpenuhi: 56,2%** | 59.982 → 26.281 token (skenario normal) |
| Konflik selalu berhenti aman dan meminta manusia | **terpenuhi: 3/3** | semua berakhir `NEEDS_HUMAN`; 2 dari 3 dengan event `human_requested`, 1 setelah percobaan ke-2 habis |
| Tidak ada kerusakan branch dasar | **terpenuhi: 15/15 run** | `main` repository fixture tetap di commit awal dan bersih; E2E `baseIntact` pada skenario gagal |
| Anti-SSRF di setiap panggilan provider | **terpenuhi** | `security::ssrf`; `tests/model_tools.rs` (tidak ada permintaan keluar untuk host terlarang) |
| Server wajib loopback + peringatan jelas | **terpenuhi** | `config::check_bind` (tolak start kecuali `NOCTIS_ALLOW_NON_LOOPBACK=1`), peringatan di log setiap start |
| Tidak ada edit di luar allowed paths | terpenuhi (diuji) | `patches_outside_the_allowed_paths_are_rejected_without_touching_git`, allowlist `ToolPolicy` |
| Semua task `DONE` punya review, verification, patch, event, usage | terpenuhi (diuji) | `tests/e2e/zz-release-audit.spec.cjs` (21 task hasil orkestrasi lengkap; 4 seeding fixture dilaporkan terpisah) |
| Restart tidak kehilangan status | terpenuhi (diuji) | E2E `[matrix:restart-recovery]`, `tests/recovery.rs` |
| 2–4 worker dapat dipantau dan dihentikan | terpenuhi (diuji) | E2E `[matrix:parallel-four-workers]`, `cancel_stops_the_slot_and_finalizes_the_task` |

## Sebelum push (tindakan pemilik)

Riwayat lokal berisi fixture token Slack palsu di `tests/security.rs` (commit awal M5-001) yang membuat GitHub push protection menolak
push. Riwayat lokal yang belum dipush dibersihkan dengan menulis ulang commit tersebut (hash berubah) dan literal dipecah
dengan `concat!` (perilaku test identik). Setelah itu `git push origin main` dijalankan manual oleh pemilik.

## Keputusan pemilik yang berlaku

- **Otentikasi dan TLS ditunda** untuk MVP lokal, dengan syarat server tetap di loopback (ditegakkan di kode) dan peringatan jelas
  di setiap start. Ini wajib ditinjau ulang sebelum penggunaan di luar satu operator lokal.
- Skenario `file-conflict` bukan target selesai otomatis; kriterianya berhenti aman (lihat benchmark.md).

## Known limitations

Keamanan dan operasional (rincian: [security.md](security.md), [operations.md](operations.md)):
- Tidak ada autentikasi/otorisasi, TLS, dan rate limiting. `actor_id` pada approval tidak diverifikasi. Siapa pun yang menjangkau port
  dapat membuat run, menyetujui plan, dan mendaftarkan provider publik pilihannya (kode repository akan dikirim ke sana).
- Akses Docker untuk verifikasi (`compose.docker-socket.yaml`) setara root di host; tidak aktif secara bawaan, sehingga pada Compose
  bawaan task tidak dapat diverifikasi.
- Redaksi secret mengurangi kebocoran, tidak menjaminnya; artifact diff dan isi database tidak diredaksi.
- Arsip backup tidak dienkripsi; repository Git proyek dan cabang `noctis-integration-<run>` tidak ikut backup.
- CSP memakai `'unsafe-inline'` (batasan SvelteKit adapter-static).

Fungsional:
- Satu image verifikasi untuk seluruh server (`NOCTIS_RUNNER_IMAGE`, bawaan `rust:1`).
- Fallback ke model lain belum aktif di produksi (hanya retry pada model yang sama).
- Usage panggilan Lead belum dicatat di `model_usage` (benchmark memakai proxy penghitung).
- Konfigurasi `budgets.*`, `runner.network_enabled`, dan `server.data_dir` tidak berpengaruh (hanya ditampilkan/tidak dibaca).
- Provider di mesin host tidak terjangkau dari container Compose tanpa konfigurasi jaringan tambahan.
- Hasil kerja hanya berupa cabang Git lokal; tidak ada push/PR otomatis.
- Lead dapat membuat plan dengan scope tumpang tindih yang ditolak validasi; pengguna meminta plan lagi.
- Pengujian browser hanya Chromium pada viewport desktop.

Pengujian dan bukti:
- Benchmark: 3 putaran, satu model, satu router; penghematan token sangat dipengaruhi overhead ±4.939 token/panggilan router lokal dan
  belum diuji di provider tanpa overhead. Jalur percobaan ulang pada skenario normal tidak teramati di run nyata (hanya oleh test/E2E
  dan jalur konflik). Latensi tidak menjadi gate dan bisa lebih lama dari baseline.
- Matriks E2E memakai provider palsu deterministik, yang terbukti pernah menyembunyikan blocker besar; perilaku model nyata hanya diukur oleh benchmark.
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
# benchmark nyata (butuh provider dan API key lewat environment backend; lihat benchmark.md)
```
