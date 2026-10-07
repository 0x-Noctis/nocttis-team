# Panduan Operator

Untuk orang yang memasang dan menjalankan Noctis Team di mesinnya. Cara memakai aplikasinya ada di
[user-guide.md](user-guide.md); masalah umum ada di [troubleshooting.md](troubleshooting.md).

## Gambaran

Satu proses `ai-team` (Rust/Axum) melayani API `/api/v1/...` dan WebApp statis dari origin yang sama, memakai satu database
PostgreSQL 16. Scheduler di dalam proses menjalankan worker secara paralel; verifikasi hasil kerja berjalan di container
Docker tanpa jaringan. Hasil akhir adalah **cabang Git lokal** `noctis-integration-<run>` di repository proyek Anda: tidak
ada `git push`, dan cabang utama tidak pernah diubah.

```
Browser ──► ai-team (API + WebApp, :7410) ──► PostgreSQL
                │  ├─ Lead/worker/reviewer ──► provider model (OpenAI-compatible, lewat internet/LAN)
                │  └─ verifikasi ──► docker run --network none (opsional, butuh akses Docker)
                └─ repository proyek (Git) + data/worktrees + data/artifacts
```

## Prasyarat

| Kebutuhan | Untuk | Catatan |
|---|---|---|
| Docker + plugin Compose | jalan Compose (disarankan) dan verifikasi task | tanpa Docker, task tidak bisa diverifikasi |
| Git | repository proyek dan cabang integrasi | sudah ada di image |
| Provider model OpenAI-compatible | Lead, worker, reviewer | wajib mendukung `chat/completions`; worker butuh tool calling |
| Rust stable + Node.js 20+ | hanya bila build tanpa Docker/untuk pengembangan | |

## Instalasi

### A. Docker Compose (satu mesin)

```bash
git clone <repository-url> noctis-team && cd noctis-team
cp .env.example .env
# Edit .env: isi PRIMARY_API_KEY (lihat tabel environment) dan, bila provider Anda di localhost/LAN,
# NOCTIS_PROVIDER_HOST_ALLOWLIST. Jangan commit .env.
docker compose up -d --build        # build pertama beberapa menit (butuh internet)
curl -s http://127.0.0.1:7410/api/v1/health/ready      # {"status":"ready",...} bila database dan migration sehat
```
Pada instalasi baru `components.providers` bernilai `degraded` sampai Anda mendaftarkan provider dan model; itu wajar,
`status` tetap `ready`.

Catatan jaringan: di dalam container, `127.0.0.1` adalah container itu sendiri. Provider yang berjalan di mesin host
(mis. router lokal) tidak terjangkau dengan `base_url` `http://127.0.0.1:…`; perlu konfigurasi jaringan tambahan
(mis. `extra_hosts: host.docker.internal:host-gateway` pada service `noctis`, belum diuji di repository ini) atau jalankan
server dengan cara B di bawah. Verifikasi task juga membutuhkan akses Docker (lihat [deployment.md](deployment.md)).
Buka `http://127.0.0.1:7410`. Migration database dijalankan otomatis saat server start. Detail image, hardening, dan opsi
akses Docker untuk verifikasi: [deployment.md](deployment.md).

### B. Tanpa Docker untuk aplikasinya (PostgreSQL tetap lewat Compose)

```bash
cp .env.example .env                       # isi PRIMARY_API_KEY
docker compose up -d --wait postgres
(cd web && npm ci && npm run build)
cargo build --release --bin ai-team
set -a; . ./.env; set +a
NOCTIS__SERVER__WEB_ROOT=web/build ./target/release/ai-team
```

## Referensi environment

Nilai secret hanya lewat environment; tidak pernah di berkas konfigurasi, image, atau log.

| Variabel | Wajib | Fungsi |
|---|---|---|
| `DATABASE_URL` | ya | koneksi PostgreSQL; server berhenti bila kosong/tidak terhubung |
| `PRIMARY_API_KEY` | ya | server menolak start bila kosong. Dipakai juga sebagai nama bawaan "API key env" saat mendaftarkan provider |
| *(variabel apa pun yang Anda namai di provider)* | per provider | kolom **API key env** di halaman Providers berisi NAMA variable; nilainya dibaca dari environment proses backend. Provider kedua dengan key lain = variable lain, ditambahkan ke environment backend |
| `PRIMARY_BASE_URL`, `PRIMARY_MODEL` | tidak | hanya dibaca `compose.yaml` dan diteruskan ke `NOCTIS__PROVIDER__BASE_URL/MODEL`. **Bukan** pendaftaran provider |
| `NOCTIS_CONFIG` | tidak | path berkas TOML ([config/example.toml](../config/example.toml)); tanpa itu dipakai nilai bawaan |
| `NOCTIS__<SECTION>__<FIELD>` | tidak | menimpa satu nilai config (tepat dua segmen, mis. `NOCTIS__SCHEDULER__MAX_PARALLEL_AGENTS=4`); nilai angka/boolean/array ditulis sebagai TOML |
| `NOCTIS_PROVIDER_HOST_ALLOWLIST` | tidak | daftar host persis (koma) yang boleh dipakai provider internal. Tanpa ini **probe** ke alamat loopback/privat/link-local ditolak (`provider_unavailable`, latensi 0). Pemeriksaan ini hanya ada di probe; panggilan Lead/worker saat run tidak memeriksanya lagi |
| `NOCTIS_RUNNER_IMAGE` | tidak | image container untuk perintah verifikasi; bawaan `rust:1`. **Satu image untuk seluruh server**, jadi proyek Node butuh image berisi Node. Pakai tag yang di-pin dari sumber tepercaya |
| `RUST_LOG` | tidak | filter log; bawaan `info` |
| `NOCTIS_PG_CONTAINER`, `NOCTIS_PG_USER`, `NOCTIS_PG_DATABASE`, `NOCTIS_BACKUP_DIR`, `NOCTIS_ARTIFACT_ROOT`, `NOCTIS_DATA_DIR`, `NOCTIS_REPOS_DIR`, `DOCKER_GID` | tidak | skrip backup/restore dan Compose opsional ([backup.md](backup.md), [deployment.md](deployment.md)) |

## Referensi konfigurasi (`config/example.toml`)

Key yang tidak dikenal atau nilai tidak valid membuat server **menolak start** dengan pesan yang jelas.

| Key | Bawaan | Berpengaruh? |
|---|---|---|
| `server.bind` | `127.0.0.1:7410` | ya |
| `server.cors_allowed_origins` | `["http://127.0.0.1:5173"]` | ya (hanya relevan bila UI di origin lain/mode dev); maksimal 8, tanpa wildcard |
| `server.web_root` | kosong | ya; kosong = hanya API |
| `server.data_dir` | `./data` | **tidak** (belum dibaca kode); pakai `git.worktree_root` dan `artifacts.root` |
| `database.max_connections`, `acquire_timeout_seconds` | 10, 5 | ya |
| `scheduler.max_parallel_agents` | 2 | ya; 1–4 diuji |
| `scheduler.heartbeat_seconds`, `stale_after_seconds` | 10, 60 | ya; attempt tanpa detak > `stale_after_seconds` dipulihkan |
| `budgets.*` (4 key) | 300000 / 30000 / 8000 / 15 | **hanya ditampilkan** di Settings; batas nyata = `token_budget` per run dan limit per task dari plan |
| `git.worktree_root` | `./data/worktrees` | ya |
| `git.retention_hours` | 24 | ya (retensi worktree, lihat bawah) |
| `artifacts.root`, `max_tool_output_bytes` | `./data/artifacts`, 1 MiB | ya; artifact lebih besar dari batas ditolak |
| `runner.network_enabled` | `false` | **hanya ditampilkan**; verifikasi selalu `--network none` |
| `provider.model` | `gpt-5-mini` | ya: ID model terdaftar yang dipakai scheduler dan Lead sebagai default |
| `provider.base_url` | OpenAI | hanya divalidasi dan hostnya ditampilkan |

> **Aturan yang paling sering terlewat:** `provider.model` (atau `NOCTIS__PROVIDER__MODEL`) harus sama persis dengan "Model ID"
> yang Anda daftarkan di halaman Providers. Scheduler menunggu model itu muncul; bila ID berbeda, task tetap `READY`
> tanpa error. Model bisa didaftarkan setelah server berjalan.

## Memantau

- **Log**: JSON, satu baris per kejadian, ke stdout (`docker compose logs -f noctis`). Per request hanya method, path,
  status, latensi, dan `request_id`; query string, header, dan body tidak dicatat. Setiap respons membawa header
  `X-Request-Id` (dipakai ulang bila klien mengirim UUID valid; selain itu dibuat server). Kumpulkan log dengan alat infrastruktur Anda.
- **Health**: `GET /api/v1/health/live` (proses hidup, tanpa DB), `/health/ready` (200 bila database + migration sehat,
  selain itu 503), `/health` (kompatibel klien lama).
- **Metrik**: `GET /api/v1/metrics` (teks Prometheus): `noctis_up`, `noctis_uptime_seconds`, `noctis_component_health`,
  `noctis_tasks{status}`, `noctis_attempts{status}`, `noctis_stale_attempts`, `noctis_models`, `noctis_tokens{kind}`, dll.
  Tetap menjawab walau database mati (seri DB dilewati).
- **UI**: halaman **Operations** (kesehatan komponen, antrean, pemakaian token, kesiapan provider, retensi) dan **Settings**
  (konfigurasi non-secret yang sedang berlaku; tanpa path server dan tanpa secret).

## Perilaku yang perlu diketahui

- **Retry dan fallback provider**: error sementara (rate limit, timeout, provider tidak tersedia) diulang dengan backoff
  eksponensial (bawaan 3 percobaan, total tidur ≤ 8 detik). Error tetap (autentikasi salah, konteks terlalu besar, budget
  habis) tidak pernah diulang. Retry ditutup bila ada efek samping tool yang hasilnya belum pasti. **Fallback ke model
  lain belum aktif di produksi** (belum ada konfigurasinya).
- **Budget**: tiap panggilan model dicek terhadap budget run secara fail-closed; bila database tidak terbaca, panggilan
  ditolak, bukan dilewatkan.
- **Restart/crash**: saat start, server memulihkan attempt yang ditinggalkan (lease file dan reservasi budget dilepas) dan
  melanjutkan integrasi yang terputus. Status task, run, dan plan ada di database sehingga tidak hilang. Pekerjaan yang
  sedang berjalan di model saat crash diulang dari attempt baru, bukan dilanjutkan di tengah panggilan.
- **Retensi** (tiap jam; pertama 5 menit setelah start): artifact yatim di disk dan direktori worktree cabang integrasi run
  yang sudah tuntas dihapus setelah `git.retention_hours`. Bukti task `DONE` (diff, verifikasi) dan **cabang Git** tidak
  dihapus.
- **Shutdown**: `SIGTERM`/`Ctrl-C` menghentikan pengambilan task baru dan menunggu slot yang berjalan (batas 10 detik).

## Backup dan restore

`scripts/backup.sh` dan `scripts/restore.sh` mencadangkan database dan artifact; restore hanya ke tujuan **kosong** dan tidak
pernah menimpa. Prosedur, kode keluar, dan batasnya: [backup.md](backup.md). Yang **tidak** ikut: repository Git proyek beserta
cabang `noctis-integration-<run>` (cadangkan terpisah, mis. `git bundle`), konfigurasi, dan secret. Arsip tidak dienkripsi.
Latih restore pada salinan data secara berkala.

## Upgrade

1. Cadangkan (lihat atas) dan catat versi/commit saat ini.
2. `git pull` (atau ambil rilis baru), lalu `docker compose up -d --build`. Migration berjalan otomatis saat start dan
   hanya menambah (append-only); server menolak start bila skema database tidak cocok dengan migration yang dikenalnya.
3. Periksa `GET /api/v1/health/ready` dan halaman Operations. Untuk rollback, restore backup ke database baru lalu pindahkan
   aplikasi versi lama ke sana; migration tidak punya langkah turun.
4. Run yang sedang berjalan saat upgrade diperlakukan seperti crash (dipulihkan saat start). Sebaiknya jeda atau tunggu run
   selesai dulu.

## Batas keamanan (jujur)

Ringkas; rincian dan pengujian di [security.md](security.md).

- **Tidak ada autentikasi API/UI.** Siapa pun yang bisa menjangkau port bisa membuat run, menyetujui plan, dan membaca
  data. Karena itu port hanya dipublikasikan ke `127.0.0.1`. Jangan ubah ke `0.0.0.0` tanpa reverse proxy yang menambah
  autentikasi dan TLS. `actor_id` pada approval dicatat tetapi tidak diverifikasi. `/metrics` dan `/health/*` juga terbuka.
- **Tidak ada TLS dan rate limiting** di aplikasi.
- **Perlindungan SSRF provider hanya pada probe.** Siapa pun yang bisa memakai API dapat mendaftarkan provider dengan
  `base_url` apa saja; panggilan model saat run tidak memeriksa tujuan lagi. Satu alasan lagi API tidak boleh diekspos.
- **Akses Docker** untuk verifikasi (`compose.docker-socket.yaml`) setara akses root ke host; sengaja tidak aktif secara bawaan.
- **Isi repository dan model dianggap tidak tepercaya**, tetapi redaksi secret hanya mengurangi kebocoran. Jangan menaruh
  secret di repository yang dikerjakan agent. Kode hasil agent harus ditinjau manusia sebelum di-merge.
- Password PostgreSQL di `compose.yaml` adalah nilai dev; ganti di luar mesin lokal.
- CSP memakai `'unsafe-inline'` untuk script (batasan SvelteKit adapter-static).
