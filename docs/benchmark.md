# Benchmark MVP terhadap Baseline

Dokumen ini membandingkan satu agent (baseline M0-004, lihat [baseline.md](baseline.md)) dengan MVP Noctis
pada **lima skenario fixture yang sama** (`tests/fixtures/sample-project/scenarios/tasks.json`).

## Status

| Bagian | Status |
|---|---|
| Harness yang bisa diulang (`mvp-run.cjs`, `compare.cjs`) | selesai dan teruji |
| Pengukuran MVP dengan model nyata (`mvp-results.json`) | **selesai (2026-10-08)**: satu run per skenario, model sama dengan baseline |
| Gate: median input token turun ≥ 50% | **TIDAK TERCAPAI**: turun 47,0% (61.118 → 32.402) |
| Gate: ≥ 80% skenario selesai tanpa intervensi | **tercapai tepat di ambang**: 4 dari 5 (80,0%) |

Keputusan rilis: aturan "median input token turun minimal 50% atau release ditahan" belum terpenuhi, jadi rilis **ditahan**
(lihat release-notes.md). Penjelasan kegagalan target ada di bagian "Hasil pengukuran nyata".

## Cara menjalankan

Prasyarat: backend dan PostgreSQL berjalan dengan **database kosong** (scheduler melayani semua run, run lain
akan ikut berjalan dan mengacaukan hitungan), Docker tersedia, dan provider OpenAI-compatible dapat dijangkau.
Backend harus dijalankan dengan:

```sh
export NOCTIS_PROVIDER_HOST_ALLOWLIST=127.0.0.1        # proxy penghitung berjalan di localhost
export NOCTIS__PROVIDER__MODEL=bench-model             # harus sama dengan NOCTIS_BENCH_MODEL_ID
export BENCH_API_KEY=<API key provider>                # nilai hanya ada di environment backend
```

Lalu, dari root repository:

```sh
NOCTIS_BENCH_UPSTREAM=https://provider.example/v1 \
NOCTIS_BENCH_MODEL=<nama model di provider> \
NOCTIS_BENCH_API_KEY_ENV=BENCH_API_KEY \
NOCTIS_BENCH_POSTGRES=<nama container postgres> \
node tests/baseline/mvp-run.cjs          # menulis tests/baseline/mvp-results.json

node tests/baseline/compare.cjs          # tabel baseline → MVP + gate (exit 0 bila kedua gate lulus)
node --test tests/baseline/*.test.cjs    # test harness (sintetis, tanpa model)
node tests/baseline/verify.mjs           # skema + total token baseline vs event mentah
```

Daftar lengkap variabel ada di komentar kepala `tests/baseline/mvp-run.cjs` (termasuk `NOCTIS_BENCH_DUMP=<berkas>` untuk mencatat request/respons ke provider saat diagnosis; header Authorization tidak pernah dicatat, tetapi isi prompt dicatat, jadi jangan dipakai pada repository sensitif). Skrip tidak pernah membaca nilai
API key; ia hanya mendaftarkan **nama** env var ke backend.

Untuk pembanding yang adil, pakai **model yang sama** dengan baseline (`td/cx/gpt-5.6-sol-review`, provider
`9router`, lihat baseline.md) atau catat dengan jelas bila berbeda, karena token per task sangat bergantung model.

## Metodologi

Satu skenario = satu project baru dari fixture (commit deterministik `reset.sh`), satu run dengan objective dari
`title` skenario + batas path, acceptance dari `acceptance` skenario, budget 400.000 token. Alurnya: Lead membuat
plan → plan disetujui → scheduler menjalankan worker/reviewer/integrator sampai run `DONE`/`FAILED`/`CANCELLED`
(atau timeout, default 15 menit). Hasil diverifikasi dengan menjalankan perintah `verify` skenario pada branch
`noctis-integration-<run>`.

| Metrik | Definisi di MVP | Baseline (M0-004) |
|---|---|---|
| Success | run `DONE` **dan** verify pada branch integrasi lulus (≥1 test, 0 gagal) | verifikasi akhir lulus |
| Input/output token | jumlah `usage` dari **semua** panggilan model (Lead + worker + reviewer), diukur proxy lokal | event `turn.completed` Codex |
| Latency | detik dari permintaan plan ke Lead sampai run terminal | durasi sesi |
| Retry | jumlah `agent_runs` dengan `attempt > 1` | retry level agent |
| Conflict | jumlah task berstatus `CONFLICT` | konflik Git yang terlihat |
| Tests | hitungan pass/fail `node --test` pada branch integrasi | hitungan test fixture |
| Intervention | aksi manusia di luar approval plan (retry/start/cancel manual); kolektor tidak melakukan satupun, jadi 0 | 0 |
| Cost | `N/A` kecuali provider melaporkan tarif | `N/A` |

Keputusan yang perlu diketahui pembaca:

- **Approval plan bukan intervensi.** Approval adalah gerbang kebijakan (RANCANGAN §24 butir 5), dicatat terpisah
  sebagai `plan_approvals` (1 per skenario). Baseline tidak punya padanannya; bila Anda menghitungnya sebagai
  intervensi, gate 80% tidak mungkin lulus oleh desain, jadi definisi ini harus disetujui pemilik.
- **Gate token memakai median per skenario**, bukan rata-rata, mengikuti kalimat RANCANGAN §23. Median baseline
  adalah 61.118 token input (tervalidasi test), sehingga target MVP ≤ 30.559.
- **Sukses diukur seragam.** Pada `test-failure` dan `file-conflict` baseline menilai "sukses" setelah agent
  memperbaiki kondisi yang disengaja; MVP dinilai dengan aturan yang sama (hasil integrasi akhir lulus test).
  Perilaku "ditolak dan branch dasar aman" untuk kasus ini sudah dicakup matriks E2E (M5-009), bukan benchmark ini.

## Temuan saat membuat harness

1. **Token Lead tidak tercatat di `model_usage`** (`src/api/lead.rs`, komentar `ponytail`). Angka dari database
   saja akan **menyepelekan** biaya MVP dan membuat gate token tampak lebih mudah lulus. Karena itu kolektor
   menghitung token lewat proxy independen dan melaporkan `lead_input_tokens = proxy − model_usage`. Pada validasi
   dengan fake provider selisihnya tepat sama dengan token panggilan Lead (24 dari 60), jadi kedua jalur konsisten.
   Rekomendasi: catat usage Lead ke `model_usage` (di luar Allowed Paths task ini, perlu keputusan pemilik).
2. **ID model harus sama dengan `NOCTIS__PROVIDER__MODEL` backend.** Bila beda, task tetap `READY` dan tidak pernah
   dijalankan scheduler tanpa pesan error yang jelas. Dicatat di prasyarat di atas.
3. `node --test` anak mewarisi `NODE_TEST_CONTEXT` bila dipanggil dari test runner dan mengubah format output;
   kolektor membuangnya agar hitungan pass/fail tidak nol secara diam-diam.

## Validasi harness (apa yang terbukti dan yang belum)

Terbukti oleh test otomatis (`tests/baseline/*.test.cjs`, 12 test, termasuk mutation check pada ambang gate dan hitungan test
gagal): perhitungan median/gate/biaya N/A, penjumlahan `usage` dari JSON dan SSE, normalisasi respons router, serta verifikasi
branch integrasi (hijau, merah, branch tidak ada).

Terbukti oleh run nyata: seluruh jalur kolektor sampai `DONE` dan verifikasi pada branch integrasi untuk 4 skenario, penulisan
`mvp-results.json`, dan pencatatan kegagalan skenario (`file-conflict`) tanpa membuang skenario lain.

**Belum terbukti:** kolom `retries` dan `conflicts` bernilai 0 di semua run nyata, jadi jalur hitungnya (attempt > 1, task
`CONFLICT`) belum teramati terisi pada data nyata; ia hanya benar menurut query. Hasil adalah satu run per skenario.

## Hasil pengukuran nyata (2026-10-08)

Dijalankan dengan router lokal `http://localhost:20128/v1`, model `td/cx/gpt-5.6-sol-review` (sama dengan baseline), database
kosong, `NOCTIS_RUNNER_IMAGE=node:22-bookworm-slim`, fixture commit `74aa0aba…`. Data mentah: `tests/baseline/mvp-results.json`
(termasuk `run_id` tiap skenario). Satu run per skenario, seperti baseline; keluaran model tidak deterministik sehingga angka
bisa bergeser antar run. Token probe kemampuan tool (penyiapan) **tidak** dihitung.

| Scenario | Success | Input tokens | Output tokens | Latency (s) | Retry | Conflict | Tests passed | Intervention |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| backend-only | yes → yes | 43.706 → 32.402 | 1.350 → 1.126 | 24 → 38 | 0 → 0 | 0 → 0 | 2 → 2 | 0 → 0 |
| frontend-only | yes → yes | 62.833 → 32.489 | 1.417 → 840 | 29 → 23 | 0 → 0 | 0 → 0 | 1 → 1 | 0 → 0 |
| cross-stack | yes → yes | 61.118 → 41.718 | 2.072 → 1.488 | 53 → 37 | 0 → 0 | 0 → 0 | 3 → 3 | 0 → 0 |
| test-failure | yes → yes | 58.846 → 31.581 | 1.125 → 1.539 | 25 → 56 | 0 → 0 | 0 → 0 | 1 → 1 | 0 → 0 |
| file-conflict | yes → **no** | 76.461 → 5.843 | 2.864 → 623 | 51 → 0 | 0 → 0 | 1 → 0 | 3 → 0 | 0 → 0 |

Biaya: N/A (router tidak melaporkan tarif; tidak diisi dengan asumsi). Ringkasan (`node tests/baseline/compare.cjs`):

| Gate | Hasil | Syarat |
|---|---:|---:|
| Median input token (per skenario) | 61.118 → 32.402 = **turun 47,0%** | ≥ 50% — **GAGAL** |
| Selesai tanpa intervensi | 4/5 = **80,0%** | ≥ 80% — lulus tepat di ambang |
| Success rate | 100% → 80% | — |

Rincian MVP (token input rata-rata skenario yang berhasil): Lead ±5,8 ribu, worker ±20–29 ribu, reviewer ±6,2 ribu (angka
worker/reviewer dari `model_usage`; Lead = total proxy − `model_usage`, `lead_input_tokens`).

### Kenapa target token tidak tercapai (dan apa yang bukan penjelasannya)

1. **Overhead router.** Router menyisipkan ±4.939 token input pada SETIAP panggilan (prompt 4 kata terukur 4.939 token). MVP
   melakukan minimal 4 panggilan per skenario (Lead, worker dengan satu giliran tool dan satu giliran akhir, reviewer), jadi
   setidaknya ±19,8 ribu dari 32,4 ribu token median (±61%) adalah overhead router, bukan isi pekerjaan. Baseline Codex juga
   membayar overhead per panggilan, tetapi dengan jumlah panggilan yang tidak kita ketahui. Akibatnya perbandingan sangat peka
   terhadap **jumlah panggilan**, dan router ini tidak mewakili provider biasa. Tidak diuji: seberapa besar rasio berubah di
   provider tanpa overhead; jangan mengklaim ia akan melewati 50%.
2. **Biaya struktur MVP.** Setiap task memerlukan panggilan Lead dan reviewer di luar pekerjaan worker; pada task kecil ini
   keduanya adalah biaya tetap. Penghematan di desain (konteks terbatas per task) baru tampak pada task besar/paralel, yang tidak
   ada di lima skenario kecil ini.
3. **Peluang penghematan yang belum dikerjakan** (usulan, bukan hasil): menyertakan isi file allowed_paths pada pesan pertama
   worker menghemat satu giliran tool (±5 ribu token di router ini); ringkasan hasil tool lebih pendek; menggabungkan Lead untuk
   task tunggal.
4. **Bukan penjelasan:** perbedaan model (sama), token probe (sudah dikeluarkan), atau data tercemar (run pertama yang menyertakan
   probe ditolak dan diulang; angkanya 38,9% hanya karena probe ikut terhitung).

### Kegagalan `file-conflict`

Pada skenario ini Lead membaca acceptance ("satu task mengubah harga 25, task lain 26") secara harfiah dan merencanakan **dua task
pada file yang sama**; validasi plan menolaknya (`lead plan contains overlapping file scopes`) dan kolektor tidak mengulang
permintaan (mengulang = intervensi). Di baseline skenario ini dicatat berhasil karena agent menyelesaikan kondisinya; di MVP perilaku
"scope tumpang tindih tidak boleh dijalankan bersamaan" adalah keputusan desain, dan konflik Git sendiri dicakup E2E
`[matrix:semantic-conflict]`. Bila manusia menekan "Ask Lead" sekali lagi, plan kemungkinan valid, tetapi itu satu intervensi.

### Temuan yang diperbaiki sepanjang pengukuran (M5-013)

Percobaan pertama dengan model nyata tidak dapat berjalan sama sekali. Penyebabnya dan perbaikannya (semua dengan test regresi):

| Temuan | Perbaikan |
|---|---|
| Router selalu SSE dan tanpa `[DONE]` | normalisasi di proxy harness (`sse-normalize.cjs`); klien produksi tidak diubah |
| Lead mengisi `context_refs` dengan path file → dispatch gagal setelah plan disetujui | referensi non-`artifact://` dibuang saat parse plan |
| Skema tool worker kosong → model memanggil tool dengan `{}` | skema lengkap per tool |
| Worker/reviewer tanpa prompt sistem; protokol penyelesaian JSON tidak dijelaskan | `prompts/worker.md`, `prompts/reviewer.md` |
| Hasil tool tidak dikembalikan ke model (hanya flag sukses) | isi hasil dikirim (dipotong 16 KiB) |
| Giliran assistant dengan `tool_calls` tidak dikirim → provider menolak pesan `tool` | `Message.tool_calls` dan serialisasi OpenAI |
| Error tool yang bisa diperbaiki (path ditolak, patch gagal) mengakhiri worker | dikembalikan ke model; hanya timeout yang fatal |
| JSON berpagar Markdown atau berkalimat pengantar ditolak | ekstraksi objek JSON (validasi tetap ketat) |
| Reviewer hanya menerima metadata diff, tanpa isi | `diff_content` (dipotong 64 KiB) |
| Reviewer menolak karena "hasil test tidak ada" padahal verifikasi berjalan sesudah review | dijelaskan di prompt reviewer |
| Batas token Lead terlalu kecil (3.000) untuk percakapan multi-giliran | panduan batas realistis di prompt Lead |
| Status run tidak pernah `DONE` | scheduler menutup run yang semua task-nya `DONE` |
| `worker.failed` tanpa sebab | sebab (jenis, tanpa isi) dicatat di log terstruktur |

## Kegagalan target

Satu gate gagal: median input token turun 47,0% (target 50%). Penjelasannya ada di "Kenapa target token tidak tercapai". Gate
tanpa intervensi lulus tepat di ambang (80,0%), tanpa margin: satu skenario lagi yang gagal akan menjatuhkannya.

## Berkas

| Berkas | Isi |
|---|---|
| `tests/baseline/results.json`, `*.jsonl` | data mentah baseline M0-004 |
| `tests/baseline/mvp-run.cjs` | kolektor MVP (proxy penghitung + run + query DB) |
| `tests/baseline/mvp-results.json` | data mentah MVP (run nyata 2026-10-08) |
| `tests/baseline/compare.cjs` | tabel perbandingan dan gate |
| `tests/baseline/compare.test.cjs`, `mvp-run.test.cjs` | test harness (data sintetis) |
