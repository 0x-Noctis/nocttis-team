# Matriks E2E deterministik

Satu perintah menjalankan seluruh matriks terhadap stack nyata (PostgreSQL di container, backend `ai-team`, fake provider
per-task, Vite + gateway, Chromium):

```bash
cd web && npm ci && npx playwright install chromium
NOCTIS_E2E_API_KEY=local-e2e-only npm run e2e          # semua spec (58 test)
NOCTIS_E2E_API_KEY=local-e2e-only npm run e2e -- matrix # hanya skenario bertag [matrix:...] (judul memuat tag)
```

Prasyarat: Docker, port 7410/7411/7412/4173/4174/55432 bebas (hapus container validasi lain di port 55432), image `rust:1`
(verifikasi task), dan `node:22-bookworm-slim` (hanya skenario Lead→eksekusi; dilewati bila tidak ada lokal — test tidak
menarik image lewat jaringan).

## Audit bukti task DONE (M5-012)

`tests/e2e/zz-release-audit.spec.cjs` berjalan terakhir pada database yang sudah diisi seluruh spec dan memastikan setiap task `DONE` yang diproses sistem
(punya event) memiliki: artifact `diff` (patch), event `status_transition` dan `verification`, usage `worker`, dan usage `reviewer` (review).
Task `DONE` tanpa event adalah seeding fixture UI dan hanya dilaporkan jumlahnya. Logika evaluasi diuji dengan baris sintetis
(satu test per jenis bukti yang hilang).

## Determinisme
- Provider palsu (`tests/e2e/fake-provider.cjs`) menjawab per task berdasarkan penanda `[ptask:<id>;kunci=nilai]` di
  objective, bukan urutan request, sehingga beberapa worker bersamaan tidak saling mengganggu. Perilaku: patch, review
  setuju/minta perubahan, `delay` (task tumpang tindih), `slowfirst` (melewati timeout provider), `fail`+`failn` (status
  HTTP disuntikkan), `tokens` (budget), `create` (file baru). `GET /stats` memberi jumlah request per task.
- Keadaan awal dibuat lewat SQL dalam satu transaksi; hasil dibaca dari DB, git, API, dan UI; tidak ada `sleep` buta —
  semua menunggu kondisi (`until`).
- `tests/e2e/runtime.cjs` menjalankan biner `ai-team` langsung dan membuka server kontrol `127.0.0.1:7412`
  (`POST /restart-backend`) untuk mematikan backend dengan SIGKILL dan menyalakannya lagi dengan env tambahan.
- Jalankan dari lingkungan bersih: runtime membuat Postgres, repositori, dan direktori data baru tiap run lalu menghapusnya.

## Skenario wajib → test

| Tag | Skenario | Spec / judul | Yang dibuktikan |
|---|---|---|---|
| `parallel-independent` | dua task independen paralel (rancangan #1) | `parallel.spec` | jendela attempt tumpang tindih, 1 attempt/task, 2 commit integrasi, `main` utuh |
| `parallel-four-workers` | empat worker (M4-010) | `parallel.spec` | dashboard "4 of 4 active", empat jendela berbagi satu saat |
| `overlap-held` | scope overlap ditahan (#2) | `parallel.spec` | task kedua tanpa attempt sampai yang pertama selesai |
| `review-retry` | review minta perubahan, attempt kedua lulus (#3) | `parallel.spec` | `failed/review.changes_requested` lalu `completed`, satu commit |
| `provider-timeout-retry` | provider timeout → retry aman (#4) | `matrix.spec` | request pertama melewati timeout 2 dtk; 1 attempt, 4 request, 1 commit |
| `provider-transient-retry` | 503 dua kali diulang | `matrix.spec` | 5 request, 1 attempt, `DONE` |
| `provider-permanent-error-no-retry` | 401 tidak diulang | `matrix.spec` | tepat 1 request, attempt `failed`, task lain tetap lanjut, tidak terintegrasi |
| `restart-recovery` | server mati dan run pulih (#5) | `matrix.spec` | SIGKILL saat worker berjalan → attempt terputus `failed`, pemulihan `completed`, 1 commit, 1 baris `integration_operations` |
| `budget-stop` | budget habis menghentikan task (#6) | `parallel.spec` | task bergantung tetap `READY` tanpa attempt (hanya level Stop yang menahan) |
| `test-failure-blocks-integration` | test gagal mencegah integrasi (#7) | `matrix.spec` | `failed/verification.failed`; hanya task yang lulus ada di cabang integrasi |
| `semantic-conflict` | konflik semantik | `parallel.spec` | patch bersih tetapi verifikasi gabungan gagal → `NEEDS_HUMAN` + `conflict_report` |
| `plan-approval` | persetujuan plan | `lead-dag.spec` | task baru ada setelah manusia menyetujui; tolak/replan/siklus/over-budget di test tetangga |
| `approval-human-decision` | keputusan manusia | `matrix.spec` | retry dari Approvals: identitas wajib, dua langkah, alasan wajib, tercatat di audit |
| `backend-frontend-parallel` | Lead → approval → eksekusi paralel | `matrix.spec` | contract → (backend ∥ frontend) → test; 4 commit integrasi; `main` utuh |
| `vertical-slice` | provider → task → hasil via UI | `vertical-smoke.spec` | alur UI penuh, SSE reconnect, tanpa kebocoran secret/path |

Test `[matrix:coverage-check]` memastikan setiap tag di `tests/scenarios/matrix.cjs` masih punya test bertag; menghapus
atau mengganti nama test tanpa memperbarui matriks membuat suite gagal.

## Celah yang diketahui (sengaja tidak diuji E2E)
- **Fallback lintas provider**: belum diaktifkan di produksi (butuh konfigurasi eksplisit karena kode repository keluar
  ke provider lain). Retry/backoff dan penghentian pada error permanen diuji E2E; logika fallback diuji di
  `tests/model_fallback.rs`.
- Eksekusi task di dalam container Docker-outside-of-Docker (`compose.docker-socket.yaml`) tidak masuk matriks; diuji manual
  (lihat `docs/deployment.md`).
- Browser selain Chromium, dan pembaca layar sungguhan.

## Bug produk yang ditemukan matriks ini
1. Patch yang membuat file di **direktori yang belum ada** ditolak (`UnsafePath`) — `safe_parent` memanggil `canonicalize` pada
   induk yang belum ada. Diperbaiki (periksa leluhur terdekat yang ada, symlink tetap ditolak); regresi di `tests/tool_policy.rs`.
2. **File baru dari patch hilang diam-diam saat integrasi**: `git apply` tidak melacak file baru sehingga `git diff <base>`
   kosong dan task `DONE` tanpa membawa file itu. Diperbaiki (`git add --intent-to-add` untuk file baru); regresi di
   `tests/git_worktree.rs`.
3. (M4-010) Retry setelah `changes_requested` gagal karena worktree attempt lama masih ada — sudah diperbaiki sebelumnya.
