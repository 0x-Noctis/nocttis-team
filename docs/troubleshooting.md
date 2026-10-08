# Troubleshooting

Format: **gejala** → penyebab → perbaikan. Untuk detail konfigurasi lihat [operations.md](operations.md).
Setiap respons API membawa `X-Request-Id` dan field `request_id` pada body error; cari id itu di log
(`docker compose logs noctis | grep <request_id>`).

## Server tidak mau start

| Gejala | Penyebab | Perbaikan |
|---|---|---|
| `PRIMARY_API_KEY is required and must come from environment` | variable kosong/tidak ada di proses backend | isi di `.env`; pada Compose pastikan `.env` ada di folder yang sama dengan `compose.yaml`. Pada jalan tanpa Docker jalankan `set -a; . ./.env; set +a` dulu |
| `DATABASE_URL is required…` atau `failed to connect to PostgreSQL` | database belum jalan / URL salah | `docker compose up -d --wait postgres`; periksa host/port/password di `DATABASE_URL` |
| `invalid runtime config` / `unknown field` | key salah ketik di TOML atau `NOCTIS__…` | hanya key di `config/example.toml` yang dikenal; override harus tepat dua segmen `NOCTIS__SECTION__FIELD` |
| `server.cors_allowed_origins tidak valid` | wildcard, path, atau > 8 origin | tulis origin persis, mis. `http://127.0.0.1:5173` |
| `scheduler.stale_after_seconds must exceed …` | nilai tidak konsisten | buat `stale_after_seconds` > `heartbeat_seconds` |
| error migration (versi/checksum tidak cocok) | database dipakai versi aplikasi lain atau migration lama diedit | jalankan versi aplikasi yang sesuai, atau restore backup ke database baru ([backup.md](backup.md)) |
| `Address already in use` | port 7410/5432 dipakai proses lain | hentikan prosesnya atau ubah `server.bind` / pemetaan port di Compose |
| Compose: `set PRIMARY_API_KEY` | `.env` belum ada | `cp .env.example .env` lalu isi |

## Halaman tidak muncul / API tidak terjangkau

| Gejala | Penyebab | Perbaikan |
|---|---|---|
| `http://127.0.0.1:7410` menjawab JSON 404, bukan UI | server berjalan tanpa `server.web_root` (mode API saja) | set `NOCTIS__SERVER__WEB_ROOT=web/build` setelah `npm run build`; image Compose sudah memasangnya |
| UI dev (`:5173`) memuat tetapi semua panggilan gagal (CORS) | origin UI tidak ada di `server.cors_allowed_origins` | tambahkan origin persisnya |
| `GET /api/v1/health/ready` → 503 | database tidak terjangkau atau migration belum selesai | lihat `docker compose ps` dan log; `/health/live` tetap 200 selama proses hidup |
| container `noctis` `unhealthy` | `ai-team healthcheck` gagal (proses macet) | `docker compose logs noctis`; restart bila perlu |

## Provider dan model

| Gejala | Penyebab | Perbaikan |
|---|---|---|
| Probe langsung `provider_unavailable` dengan latensi 0 | host provider loopback/privat dan belum diizinkan | set `NOCTIS_PROVIDER_HOST_ALLOWLIST=<host>` (host persis, tanpa port) lalu restart |
| Probe `authentication_failed` | variable yang dinamai di "API key env" kosong/salah di environment **backend** | pastikan variable itu ada di proses backend (pada Compose: tambahkan ke `environment:` service `noctis`) dan nilainya benar |
| Probe `rate_limited` / `timeout` | kuota atau jaringan provider | tunggu/naikkan batas waktu request provider; retry otomatis hanya untuk panggilan run |
| Probe tools bukan `supported` | model tidak mendukung tool calling | pakai model lain; worker tidak bisa memakai model tanpa tool calling |
| **Task tetap `READY`/`Queued`, tidak pernah berjalan, tanpa error** | tidak ada model terdaftar dengan ID = `provider.model` | samakan "Model ID" di Providers dengan `NOCTIS__PROVIDER__MODEL` (bawaan `gpt-5-mini`), atau ubah konfigurasi lalu restart |
| `model is not registered` saat Ask Lead | sama seperti di atas | daftarkan model dengan ID itu |
| Run macet setelah provider dihapus | scheduler butuh model terdaftar | daftarkan ulang provider/model yang sama |

## Run dan task

| Gejala | Penyebab | Perbaikan |
|---|---|---|
| "Approval will be refused" / 409 saat approve | reservasi token plan melebihi sisa budget | naikkan token budget run baru, atau minta Lead plan yang lebih kecil |
| "lead plan contains overlapping file scopes" | Lead merencanakan dua task pada file yang sama (mis. membaca acceptance secara harfiah) | **Ask Lead** lagi atau tulis objective sebagai satu perubahan pada file itu; scope tumpang tindih sengaja tidak dijalankan |
| Task gagal dengan `worker.failed` | worker berhenti (error model, error tool fatal, batas token/panggilan/giliran tercapai) | cari log JSON `"message":"worker gagal"` berisi `stop_reason` dan jenis error (tanpa isi prompt); naikkan batas task lewat objective/budget bila `budget_exhausted` |
| "lead plan is invalid" | Lead menghasilkan plan siklik/tidak valid | **Ask Lead** lagi atau perjelas objective |
| "lead plan exceeds run token budget" | plan terlalu besar untuk budget | naikkan budget atau persempit objective |
| Task gagal verifikasi dengan pesan Docker/container | Docker tidak tersedia untuk server (mis. Compose tanpa override socket) | aktifkan akses Docker ([deployment.md](deployment.md), `compose.docker-socket.yaml`) atau jalankan server langsung di host yang punya Docker |
| Verifikasi gagal `command not found` (mis. `npm`) | `NOCTIS_RUNNER_IMAGE` bawaan `rust:1` tidak berisi toolchain proyek | set `NOCTIS_RUNNER_IMAGE` ke image yang sesuai (mis. `node:22-bookworm-slim`) |
| Task menunggu "Waiting for …" lama | menunggu dependency, atau path-nya dipegang task lain | normal; lihat panel antrean. Tanpa kemajuan: periksa task yang ditunggu |
| Task `CONFLICT` / `NEEDS_HUMAN` | patch bertabrakan saat integrasi / gagal berulang | buka **Approvals**, tinjau diff, lalu Retry atau Cancel |
| Run tidak bergerak setelah restart server | recovery berjalan saat start; attempt basi dipulihkan setelah `stale_after_seconds` | tunggu ± `stale_after_seconds`; cek log `startup recovery` dan halaman Operations |
| Terlalu banyak token terpakai | percobaan ulang, plan besar | kecilkan objective/budget; tinjau pemakaian di Operations |

## Disk dan data

| Gejala | Perbaikan |
|---|---|
| `data/worktrees` membesar | retensi membersihkan worktree run yang sudah tuntas setelah `git.retention_hours` (tiap jam); turunkan nilainya bila perlu. Cabang Git tidak dihapus |
| Artifact yatim | dibersihkan otomatis oleh retensi; cek halaman Operations → Retention |
| Perlu memulihkan data | [backup.md](backup.md): restore hanya ke database/direktori **kosong** |
| `backup.sh` keluar dengan 3 | secret terdeteksi di data; periksa file yang disebut (hanya nama, bukan nilai) |

## Meminta bantuan

Sertakan: versi/commit, potongan log (sudah tanpa secret) dengan `request_id`, output `GET /api/v1/health/ready`, dan
halaman Operations. **Jangan** menempelkan `.env`, API key, atau isi database.
