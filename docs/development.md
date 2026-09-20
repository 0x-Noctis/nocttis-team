# Pengembangan Noctis Team

## Alur Kontribusi

1. Pilih task dengan seluruh dependency selesai di `docs/task-list.md`.
2. Buat branch dan worktree khusus dari base commit yang ditentukan Integrator.
3. Baca acceptance criteria, verification, `Allowed Paths`, dan bagian terkait di `docs/RANCANGAN.md`.
4. Pastikan ownership file tidak bertabrakan dengan task aktif.
5. Kerjakan hanya task dan path yang diizinkan.
6. Jalankan verifikasi spesifik task, lalu gate relevan.
7. Periksa `git diff --check`, `git status --short`, dan diff sebelum commit.
8. Commit dengan format `[TASK-ID] ringkasan singkat`, lalu kirim handoff.

Integrator sendiri mengubah tracker, menggabungkan branch, dan menandai task selesai. Agent berhenti dengan `BLOCKED` jika dependency belum selesai atau `NEEDS_REVIEW` jika keputusan memengaruhi kontrak atau scope agent lain.

## Setup

Ikuti setup lokal di [`README.md`](../README.md). Gunakan `.env.example` sebagai daftar variable, simpan nilai lokal hanya di `.env`, dan jangan menyalin secret ke fixture, log, artifact, atau handoff.

Probe memblokir alamat loopback, private, dan link-local secara default. Untuk provider internal tepercaya, set `NOCTIS_PROVIDER_HOST_ALLOWLIST` ke daftar hostname exact yang dipisahkan koma.

## Validasi

Jalankan command `Verify` pada task. Gate repository lengkap:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
(cd web && npm ci && npm run check && npm run build)
docker compose config --quiet
```

Test integrasi PostgreSQL, bila diminta task:

```sh
docker compose up -d postgres
cargo test --test integration -- --test-threads=1
```

Jangan memperbaiki kegagalan di luar scope task. Catat kegagalan tersebut sebagai risiko atau follow-up.

## Handoff

```text
Task: TASK-ID — judul
Status: READY_FOR_REVIEW | BLOCKED | NEEDS_REVIEW
Branch: nama branch
Base commit: hash commit dasar
Commit: hash commit hasil
Files changed: daftar path
Acceptance criteria: hasil setiap kriteria
Commands run: command yang benar-benar dijalankan
Test results: PASS/FAIL dan ringkasan
Decisions: keputusan lokal dalam scope
Known risks: risiko tersisa
Blocker/follow-up: tindakan lanjutan
```

Handoff tidak menggantikan artifact task. Sertakan referensi patch, hasil test, atau artifact lain bila task menghasilkannya.
