# AI Team

Fondasi WebApp tim AI coding: Rust/Axum, SvelteKit, PostgreSQL, dan OpenAI-compatible API.

## Menjalankan

```sh
cp .env.example .env
docker compose up -d postgres
set -a; source .env; set +a
cargo run
```

Terminal kedua:

```sh
cd web
npm install
npm run dev
```

Buka `http://127.0.0.1:5173`. Probe mengirim request kecil ke provider yang dikonfigurasi.

## Pemeriksaan

```sh
cargo test
cd web && npm run check
```

