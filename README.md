# Noctis Team

Fondasi WebApp tim AI coding: Rust/Axum, SvelteKit, PostgreSQL, dan OpenAI-compatible API.

## Prasyarat

- Rust stable
- Node.js 20+ dan npm
- Docker dengan Compose plugin

## Setup Lokal

```sh
git clone <repository-url> noctis-team
cd noctis-team
cp .env.example .env
docker compose up -d --wait postgres
```

Isi `PRIMARY_API_KEY` di `.env` untuk probe provider. Jangan commit `.env` atau credential lain.

Jalankan backend:

```sh
set -a
. ./.env
set +a
cargo run
```

Jalankan WebApp dari terminal kedua:

```sh
cd web
npm ci
npm run dev
```

Buka `http://127.0.0.1:5173`. Backend tersedia di `http://127.0.0.1:7410`; health check berada di `/api/v1/health`. Probe mengirim request kecil ke provider yang dikonfigurasi dan dapat memakai kuota provider.

Hentikan layanan lokal dengan `Ctrl-C`, lalu `docker compose down`.

## Validasi

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
(cd web && npm ci && npm run check && npm run build)
docker compose config --quiet
```

Panduan kontribusi, aturan agent, dan format handoff berada di [`docs/development.md`](docs/development.md) dan [`AGENTS.md`](AGENTS.md).
