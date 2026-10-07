# Noctis Team

WebApp tim AI coding: Anda memberi tujuan untuk sebuah repository Git, **Lead** menyusun plan, Anda menyetujuinya, lalu
2–4 **worker** mengerjakannya paralel di worktree terpisah, direview, diverifikasi di container tanpa jaringan, dan
digabung ke cabang lokal `noctis-integration-<run>`. Noctis tidak pernah `git push` dan tidak mengubah cabang utama.
Stack: Rust/Axum, SvelteKit, PostgreSQL 16, provider model OpenAI-compatible.

> **Batas yang harus dibaca dulu:** API dan UI **tidak punya autentikasi** (dan tidak ada TLS). Port hanya dipublikasikan ke
> `127.0.0.1`; jangan diekspos ke jaringan tanpa reverse proxy berautentikasi. Rincian jujur: [docs/security.md](docs/security.md).

## Mulai cepat (Docker Compose)

Prasyarat: Docker dengan plugin Compose, dan satu provider model OpenAI-compatible beserta API key-nya.

```sh
git clone <repository-url> noctis-team && cd noctis-team
cp .env.example .env            # isi PRIMARY_API_KEY (nilai API key provider); jangan commit .env
docker compose up -d --build    # build pertama beberapa menit
curl -s http://127.0.0.1:7410/api/v1/health/ready      # {"status":"ready",...}
```

Buka `http://127.0.0.1:7410`, lalu ikuti [panduan pengguna](docs/user-guide.md): daftarkan provider dan model
(**Model ID harus sama dengan `PRIMARY_MODEL`**), daftarkan repository, mulai run, setujui plan.
Agar task dapat **diverifikasi**, server perlu akses Docker; opsi dan risikonya ada di [docs/deployment.md](docs/deployment.md).

## Dokumentasi

| Untuk | Dokumen |
|---|---|
| Memakai aplikasi (provider → project → run → approval → hasil) | [docs/user-guide.md](docs/user-guide.md) |
| Memasang, mengonfigurasi, memantau, upgrade, batas keamanan | [docs/operations.md](docs/operations.md) |
| Masalah umum dan solusinya | [docs/troubleshooting.md](docs/troubleshooting.md) |
| Image produksi dan Compose | [docs/deployment.md](docs/deployment.md) |
| Backup dan restore | [docs/backup.md](docs/backup.md) |
| Model ancaman dan kontrol keamanan | [docs/security.md](docs/security.md) |
| Perbandingan dengan single-agent | [docs/baseline.md](docs/baseline.md), [docs/benchmark.md](docs/benchmark.md) |
| Rancangan dan status pekerjaan | [docs/RANCANGAN.md](docs/RANCANGAN.md), [docs/task-list.md](docs/task-list.md) |
| Berkontribusi, matriks test, aturan agent | [docs/development.md](docs/development.md), [docs/test-matrix.md](docs/test-matrix.md), [AGENTS.md](AGENTS.md) |

## Pengembangan lokal (tanpa image produksi)

Prasyarat: Rust stable, Node.js 20+ dan npm, Docker dengan Compose plugin.

```sh
cp .env.example .env            # isi PRIMARY_API_KEY
docker compose up -d --wait postgres
set -a; . ./.env; set +a
cargo run                       # backend di http://127.0.0.1:7410
```

Terminal kedua:

```sh
cd web && npm ci && npm run dev  # UI di http://127.0.0.1:5173
```

Hentikan dengan `Ctrl-C`, lalu `docker compose down`.

## Validasi

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -- --test-threads=1   # test integrasi memakai PostgreSQL pada port 55432 bila tersedia
(cd web && npm ci && npm run check && npm run build)
PRIMARY_API_KEY=dummy docker compose config --quiet   # Compose mewajibkan variable ini terisi; nilai dummy cukup untuk validasi
```

Test yang memakai database (`#[sqlx::test]`) membaca `DATABASE_URL` dan butuh role superuser. Drill backup/restore juga
membutuhkan container PostgreSQL yang menerbitkan port 55432 **dengan superuser bernama `postgres`**; langkah lengkapnya di
[docs/development.md](docs/development.md). Tanpa container itu test backup dilewati.

E2E browser (`cd web && npm run e2e`) dan benchmark dijelaskan di [docs/test-matrix.md](docs/test-matrix.md) dan
[docs/benchmark.md](docs/benchmark.md).
