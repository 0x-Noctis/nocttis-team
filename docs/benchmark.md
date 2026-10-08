# Benchmark MVP terhadap Baseline

Dokumen ini membandingkan satu agent (baseline M0-004, lihat [baseline.md](baseline.md)) dengan MVP Noctis
pada **lima skenario fixture yang sama** (`tests/fixtures/sample-project/scenarios/tasks.json`).

## Status

| Bagian | Status |
|---|---|
| Harness yang bisa diulang (`mvp-run.cjs`, `compare.cjs`) | **Selesai dan teruji** |
| Pipa data diuji end-to-end dengan fake provider | Selesai (hanya validasi alat, bukan pengukuran) |
| Pengukuran MVP dengan model nyata (`mvp-results.json`) | **TIDAK DAPAT DIJALANKAN**: percobaan 2026-10-08 menemukan bahwa worker belum berfungsi dengan model nyata (lihat "Percobaan run nyata") |
| Gate: median input token turun ≥ 50% | **Belum diketahui** |
| Gate: ≥ 80% skenario selesai tanpa intervensi | **Belum diketahui** |

Tidak ada angka MVP di dokumen ini karena belum ada pengukuran dengan model nyata. Angka dari fake provider
sengaja **tidak** dimasukkan: token-nya rekaan dan akan menyesatkan sebagai "hasil".

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

Terbukti oleh test otomatis (`tests/baseline/*.test.cjs`, 8 test, termasuk mutation check pada ambang gate dan
pada hitungan test gagal): perhitungan median/gate/biaya N/A, penjumlahan `usage` dari JSON dan SSE, serta
verifikasi branch integrasi (hijau, merah, branch tidak ada).

Terbukti oleh satu run kolektor lawan stack E2E dengan fake provider: pendaftaran provider+model, pembuatan
project/run, Lead → approve, polling, proxy penghitung, query `model_usage`/`agent_runs`, dan penulisan JSON.

**Belum terbukti:** run yang mencapai `DONE` lalu lolos verify di dalam kolektor (fake provider tidak punya skrip
untuk fixture ini, jadi semua run berakhir timeout). Jalur itu hanya dicakup test unit `verifyIntegration`.
Run model nyata pertama harus diamati untuk memastikan kolom `retries`/`conflicts` terisi masuk akal.

## Percobaan run nyata (2026-10-08)

Dijalankan dengan router lokal `http://localhost:20128/v1`, model `td/cx/gpt-5.6-sol-review` (sama dengan baseline), database
kosong, `NOCTIS_RUNNER_IMAGE=node:22-bookworm-slim`. **Tidak ada angka MVP yang dihasilkan** karena run berhenti pada
dispatch pertama. Ini bukan "target tidak tercapai", melainkan MVP belum dapat dijalankan dengan model nyata. Temuan, berurutan:

1. **Router tidak mematuhi kontrak OpenAI.** Ia selalu membalas SSE walau `stream` tidak diminta dan tidak mengirim `data: [DONE]`,
   sehingga probe chat gagal `invalid_response` dan probe streaming `provider_unavailable`. Ditangani di harness (bukan di klien
   produksi): proxy penghitung menormalkan respons (`tests/baseline/sse-normalize.cjs`, diuji). Perilaku ini juga perlu diketahui
   operator yang memakai router sejenis.
2. **Router menyisipkan ±4,9 ribu token input pada setiap panggilan** (prompt 4 kata terhitung 4.939 token). Baseline Codex memakai
   router yang sama sehingga ikut menanggung biaya ini, tetapi MVP melakukan banyak panggilan kecil per task, jadi overhead per
   panggilan memengaruhi perbandingan secara tidak setara. Interpretasi hasil nanti harus memperhitungkannya.
3. **Bug produk (diperbaiki): `context_refs` dari Lead.** Model nyata mengisinya dengan path file; `ContextBuilder` hanya menerima
   `artifact://<id>`, sehingga dispatch gagal `orchestrator.context` setelah plan disetujui. Sekarang dibuang saat parse plan
   (`src/agent/lead.rs`, regresi di `tests/lead_planner.rs`). Fake provider tidak pernah memicunya.
4. **Blocker produk (BELUM diperbaiki): worker tidak dapat bekerja dengan model nyata.** Bukti dari request sebenarnya (dump proxy):
   - Skema tool kosong: `"parameters": {"type": "object"}` dengan deskripsi "Structured read_file tool" (`src/agent/worker.rs`
     `tool_definitions`). Model tidak tahu argumen apa pun dan memanggil `read_file` dengan `{}` (dua kali), lalu worker gagal
     `worker.failed`.
   - Tidak ada prompt sistem untuk worker maupun reviewer (hanya Lead yang punya, `src/agent/prompts/lead.md`); protokol penyelesaian
     (JSON `summary`/`status`) tidak pernah dijelaskan ke model.
   - Hasil tool tidak dikembalikan ke model: `tool_message` hanya berisi `{"succeeded":…}`, bukan isi file/diff/hasil pencarian.
   - `worker.failed` tidak menyimpan sebab (tidak ada payload event/log), sehingga kegagalan seperti ini sulit didiagnosis.
   Seluruh suite E2E lulus karena fake provider tidak memeriksa skema tool, tidak membutuhkan prompt, dan tidak memakai hasil tool.
   Artinya E2E membuktikan orkestrasi, bukan kemampuan agent dengan model sungguhan.

Konsekuensi: kriteria rilis token ≥ 50% dan ≥ 80% tanpa intervensi tidak dapat dinilai sebelum protokol worker/reviewer untuk model
nyata dibuat (skema tool lengkap, prompt sistem worker dan reviewer, pengembalian hasil tool, penyimpanan sebab kegagalan) dan
diuji terhadap model nyata, bukan hanya fake provider.

## Kegagalan target

Belum berlaku: belum ada pengukuran. Bila gate gagal pada pengukuran nyata, bagian ini wajib diisi dengan
penjelasan per skenario (token per peran: Lead/worker/reviewer dari `lead_input_tokens`, `db_input_tokens`,
dan `cached_input_tokens`) sebelum release gate M5-012 dinilai.

## Berkas

| Berkas | Isi |
|---|---|
| `tests/baseline/results.json`, `*.jsonl` | data mentah baseline M0-004 |
| `tests/baseline/mvp-run.cjs` | kolektor MVP (proxy penghitung + run + query DB) |
| `tests/baseline/mvp-results.json` | data mentah MVP — **belum ada** sampai pengukuran nyata dijalankan |
| `tests/baseline/compare.cjs` | tabel perbandingan dan gate |
| `tests/baseline/compare.test.cjs`, `mvp-run.test.cjs` | test harness (data sintetis) |
