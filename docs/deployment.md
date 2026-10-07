# Deployment (image produksi dan Compose)

Satu image menjalankan satu proses `ai-team` yang melayani API (`/api/v1/...`) dan WebApp statis dari origin yang sama.
Database PostgreSQL berjalan sebagai service terpisah.

## Yang dibangun
- `Dockerfile` multi-stage: (1) `node` membangun WebApp (`npm run build`), (2) `rust` membangun biner rilis dengan `--locked`,
  (3) `debian:bookworm-slim` hanya membawa biner, WebApp hasil build, `git`, dan CLI `docker`. Tidak ada kode sumber,
  `node_modules`, `target`, atau berkas `.env` di image akhir.
- Proses berjalan sebagai user non-root `noctis` (uid/gid 10001). `HEALTHCHECK` memanggil `ai-team healthcheck`
  (liveness `/api/v1/health/live`; tidak bergantung pada DB atau provider).
- `compose.yaml`: service `postgres` dan `noctis`. Service `noctis` memakai `read_only: true`, `tmpfs: /tmp`,
  `cap_drop: ALL`, `no-new-privileges`, dan volume `noctis-data` di `/data` (worktree dan artifact).

## Menjalankan (pengembangan lokal / satu mesin)
```bash
cp .env.example .env            # isi PRIMARY_API_KEY (nilai kunci provider); .env tidak masuk git atau image
docker compose up -d --build    # Compose membaca .env otomatis
curl -s http://127.0.0.1:7410/api/v1/health/ready      # 200 bila database dan migration sehat
# buka http://127.0.0.1:7410
```
`PRIMARY_API_KEY` wajib; tanpa itu Compose menolak start dengan pesan yang jelas.

### Smoke browser
```bash
cd web && npm ci && npx playwright install chromium    # sekali
node scripts/compose-smoke.mjs                          # terhadap http://127.0.0.1:7410
```

## Jaringan dan keamanan (bacalah)
- Port dipublikasikan **hanya ke 127.0.0.1** (`127.0.0.1:7410:7410`). Di dalam container server mendengarkan `0.0.0.0` agar
  port yang dipetakan dapat dijangkau; pembatasannya ada di pemetaan port. **API belum punya autentikasi**: jangan
  mengubah pemetaan menjadi `0.0.0.0` atau mengekspos port tanpa reverse proxy yang menambah autentikasi dan TLS.
- Origin CORS bawaan hanya `http://127.0.0.1:5173` (mode dev Vite). Pada image produksi UI dan API satu origin sehingga
  CORS tidak diperlukan; ubah `server.cors_allowed_origins` hanya bila UI di-host terpisah.
- Header yang dipasang pada semua respons: `Content-Security-Policy`, `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Permissions-Policy`. CSP memakai `'unsafe-inline'` untuk script/style
  karena SvelteKit (adapter-static) menyisipkan skrip bootstrap inline; menggantinya dengan `kit.csp` (hash/nonce) adalah
  perbaikan lanjutan.
- Rahasia hanya lewat environment (`PRIMARY_API_KEY`); tidak ada di image maupun di `compose.yaml`. Password PostgreSQL di
  `compose.yaml` adalah nilai pengembangan (`ai_team_dev`); ganti untuk penggunaan di luar mesin lokal dan jangan publikasikan
  port 5432 ke jaringan.

## Menjalankan task (runner verifikasi) di dalam container — OPT-IN
Verifikasi task memakai container Docker. Dari dalam container `noctis`, itu berarti memberi akses ke daemon Docker host.
**Akses ke `/var/run/docker.sock` setara root di host**; karena itu dipisahkan di `compose.docker-socket.yaml` dan tidak aktif
secara bawaan. Tanpa itu API/UI berjalan penuh, tetapi task tidak dapat diverifikasi.
```bash
export DOCKER_GID="$(stat -c %g /var/run/docker.sock)"
export NOCTIS_DATA_DIR=/srv/noctis/data NOCTIS_REPOS_DIR=/srv/noctis/repos
sudo install -d -o 10001 -g 10001 "$NOCTIS_DATA_DIR" "$NOCTIS_REPOS_DIR"
docker compose -f compose.yaml -f compose.docker-socket.yaml up -d --build
```
Direktori data dan repository dipasang di path yang SAMA dengan di host, karena runner meneruskan path worktree ke daemon host.
Repository proyek yang didaftarkan harus berada di bawah `NOCTIS_REPOS_DIR` dan dapat ditulis uid 10001.

## Tanpa Docker
`cargo build --release --bin ai-team` dan `(cd web && npm ci && npm run build)`, lalu jalankan dengan
`NOCTIS__SERVER__WEB_ROOT=web/build` (lihat `docs/development.md`). `server.web_root` kosong = hanya API (mode dev dengan Vite).

## Batas yang diketahui
- Tidak ada TLS, autentikasi, atau rate limiting; semuanya tanggung jawab proxy di depan bila diekspos.
- Image berbasis Debian (bukan distroless) karena butuh `git` dan CLI `docker`.
- Build memerlukan akses jaringan (npm dan crates.io); build tanpa jaringan belum didukung (tidak ada vendoring).
- Backup/restore: lihat `docs/backup.md`; volume `noctis-data` dan database harus dicadangkan bersama.
