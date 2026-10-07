# Task List MVP — Noctis Team

Dokumen ini menjadi tracker utama pengerjaan MVP. Rancangan teknis lengkap berada di [`RANCANGAN.md`](./RANCANGAN.md).

## 1. Cara Memakai Tracker

### Status

- `[ ]` belum dikerjakan
- `[~]` sedang dikerjakan
- `[x]` selesai dan sudah diverifikasi
- `[!]` blocked; tulis alasan pada Catatan Aktif
- `[-]` dibatalkan; tulis alasan

Hanya Integrator yang menandai `[x]` setelah acceptance criteria dan command verifikasi lulus.

### Protokol Klaim Task

Sebelum mulai, agent wajib:

1. memilih task berstatus `[ ]` dengan seluruh dependency `[x]`;
2. memastikan `Parallel With` tidak menyentuh file sama;
3. mengubah status menjadi `[~]`;
4. mengisi satu baris pada Catatan Aktif;
5. hanya mengubah `Allowed Paths`;
6. menyerahkan ringkasan, diff, dan hasil command verifikasi;
7. tidak mengubah status task agent lain.

Format Catatan Aktif:

```text
| TASK-ID | agent-id | started-at | branch/worktree | status/blocker |
```

### Aturan Paralel

- Maksimal empat agent implementasi aktif.
- Satu file hanya dimiliki satu task aktif.
- Perubahan `Cargo.toml`, `Cargo.lock`, `web/package.json`, migration lama, dan shared API contract dilakukan Integrator atau task khusus yang tercantum.
- Migration bersifat append-only. Jangan mengubah migration yang sudah pernah dijalankan; buat file baru.
- Agent frontend boleh memakai fixture/mock sesuai kontrak API tanpa menunggu backend selesai.
- Agent QA menulis test dari kontrak, bukan detail implementasi agent lain.
- Konflik semantik masuk `[!]`; jangan diselesaikan dengan memilih perubahan secara acak.

### Handoff Wajib

```text
Task:
Status:
Files changed:
Decisions:
Commands run:
Results:
Known risks:
Follow-up:
```

## 2. Pembagian Lane

| Lane | Fokus | Direktori utama |
|---|---|---|
| A — Control Plane | domain, PostgreSQL, HTTP API, scheduler | `src/domain/`, `src/store/`, `src/api/`, `migrations/` |
| B — Execution Plane | model gateway, context, tools, Git, agent runtime | `src/model/`, `src/runner/`, `src/context/` |
| C — WebApp | Svelte UI, client API, realtime | `web/src/` |
| D — Quality & Platform | fixtures, integration test, container, docs, integration | `tests/`, `compose.yaml`, `docs/`, root config |

Agent dapat berpindah lane antargelombang. Dalam satu gelombang, pertahankan ownership agar handoff kecil.

## 3. Definition of Done Global

Task hanya selesai jika:

- acceptance criteria task terpenuhi;
- test baru mencakup logic non-trivial;
- tidak ada secret dalam source, DB, artifact, atau log;
- tidak ada edit di luar `Allowed Paths`;
- error memiliki kode stabil dan pesan aman;
- migration dapat dijalankan dari database kosong;
- `cargo fmt --check` dan test terkait lulus untuk perubahan Rust;
- `npm run check` dan test/build terkait lulus untuk perubahan WebApp;
- dokumentasi berubah jika contract atau cara operasi berubah;
- hasil verifikasi tercatat pada handoff.

## 4. Gerbang Validasi

Jalankan yang relevan pada setiap task. Integrator menjalankan seluruh command pada akhir milestone.

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cd web && npm run check
cd web && npm run build
docker compose config --quiet
```

Integration test yang memerlukan PostgreSQL:

```sh
docker compose up -d postgres
cargo test --test integration -- --test-threads=1
```

## 5. Status Ringkas

| Milestone | Tujuan | Status | Exit Gate |
|---|---|---|---|
| M0 | Baseline dan fondasi repo | `[x]` | lingkungan repeatable, baseline tersimpan |
| M1 | Provider foundation | `[x]` | provider dikelola dan seluruh probe lulus |
| M2 | Single-worker vertical slice | `[x]` | satu task menghasilkan patch terverifikasi |
| M3 | Lead Agent dan task DAG | `[x]` | plan disetujui dan dependency dipatuhi |
| M4 | Parallel workers dan integrasi | `[ ]` | 2–4 task independen berjalan aman |
| M5 | Hardening dan MVP release | `[ ]` | recovery, security, E2E, docs lulus |

## 6. Pekerjaan yang Sudah Ada

- [x] **BOOT-001 — Scaffold Rust/Axum dan SvelteKit**
  - Bukti: backend compile, frontend check/build lulus.
- [x] **BOOT-002 — PostgreSQL Compose dan migration awal**
  - Bukti: health endpoint lulus dan 10 tabel terbentuk.
- [x] **BOOT-003 — Probe chat OpenAI-compatible minimum**
  - Bukti: request normal, parser respons, dan unit test URL tersedia.
- [x] **BOOT-004 — Dokumen rancangan arsitektur**
  - Bukti: `docs/RANCANGAN.md`.

Fondasi ini belum memenuhi M1; registry provider, streaming, tool call, persistence capability, dan error taxonomy masih belum dibuat.

---

# M0 — Baseline dan Fondasi Repo

## Gelombang M0-A — Dapat Dikerjakan Bersamaan

- [x] **M0-001 — Governance repository** · Lane D
  - Depends On: —
  - Parallel With: M0-002, M0-003
  - Allowed Paths: `.gitignore`, `AGENTS.md`, `README.md`, `docs/development.md`
  - Output:
    - inisialisasi Git bila belum ada;
    - aturan kontribusi, format handoff, dan larangan secret;
    - command setup dan validasi lokal yang akurat.
  - Acceptance:
    - hasil build dan secret diabaikan;
    - developer baru dapat menjalankan stack dari README;
    - tidak ada file generated besar yang tracked.
  - Verify: `git status --short --ignored`, ikuti langkah README dari shell bersih.

- [x] **M0-002 — Repository fixture E2E** · Lane D
  - Depends On: —
  - Parallel With: M0-001, M0-003
  - Allowed Paths: `tests/fixtures/sample-project/**`
  - Output: repository Git kecil berisi backend, frontend, test lulus, dan beberapa issue terukur.
  - Acceptance:
    - fixture dapat diinisialisasi ulang secara deterministik;
    - memiliki task backend-only, frontend-only, lintas stack, test failure, dan file conflict;
    - tidak membutuhkan network saat test.
  - Verify: script/command fixture menjalankan seluruh test dengan status lulus sebelum mutasi skenario.

- [x] **M0-003 — Konfigurasi runtime typed** · Lane A
  - Depends On: —
  - Parallel With: M0-001, M0-002
  - Allowed Paths: `src/config.rs`, `src/main.rs`, `config/example.toml`, `.env.example`, `Cargo.toml`, `Cargo.lock`
  - Output: config TOML + environment override tervalidasi untuk server, DB, scheduler, budget, Git, artifact, dan provider bootstrap.
  - Acceptance:
    - startup gagal cepat pada config invalid;
    - secret hanya berasal dari environment;
    - default aman: bind localhost, dua worker, network runner nonaktif;
    - pesan error menyebut field tanpa membocorkan nilai secret.
  - Verify: unit test config valid, env override, missing secret, dan invalid limit.

## Gelombang M0-B

- [x] **M0-004 — Baseline single-agent** · Lane D
  - Depends On: M0-002
  - Parallel With: M0-005
  - Allowed Paths: `docs/baseline.md`, `tests/baseline/**`
  - Output: hasil lima skenario fixture memakai satu agent.
  - Acceptance: catat success rate, input/output token, latency, retry, konflik, test pass rate, intervensi manusia, dan biaya.
  - Verify: tabel baseline memiliki command, model, tanggal, dan artifact mentah yang dirujuk.

- [x] **M0-005 — Error envelope dan request ID** · Lane A
  - Depends On: M0-003
  - Parallel With: M0-004
  - Allowed Paths: `src/api/error.rs`, `src/api/mod.rs`, `src/main.rs`, `tests/api_error.rs`
  - Output: satu bentuk error API dan request correlation ID.
  - Acceptance:
    - error berbentuk `{ "error": { "code", "message", "details", "request_id" } }`;
    - internal cause hanya masuk structured log;
    - status HTTP dan error code stabil.
  - Verify: test 400, 404, 409, 422, 429, dan 500.

### Exit Gate M0

- [x] Semua task M0 selesai.
- [x] Fixture dan baseline dapat diulang.
- [x] Config production-safe dan error API konsisten.
- [x] Full validation lulus.

---

# M1 — Provider Foundation

## Gelombang M1-A — Contract dan Storage

- [x] **M1-001 — Domain provider dan model** · Lane A
  - Depends On: M0-003, M0-005
  - Parallel With: M1-002, M1-003
  - Allowed Paths: `src/domain/provider.rs`, `src/domain/mod.rs`, `src/api/contracts/provider.rs`
  - Output: tipe provider, model, capability, probe result, dan DTO tanpa secret value.
  - Acceptance:
    - `api_key_env` menyimpan nama environment variable saja;
    - URL wajib HTTP(S), model name non-empty, limit positif;
    - capability membedakan claimed dan verified.
  - Verify: unit test seluruh validasi boundary.

- [x] **M1-002 — Migration provider registry** · Lane A
  - Depends On: M0-003
  - Parallel With: M1-001, M1-003
  - Allowed Paths: `migrations/0002_provider_registry.sql`, `tests/migrations.rs`
  - Output: tabel `providers`, `models`, dan `provider_probes` beserta index/constraint.
  - Acceptance:
    - secret value tidak memiliki kolom;
    - delete provider menjaga referential integrity;
    - remote model name unik per provider.
  - Verify: migration database kosong dan schema constraint tests.

- [x] **M1-003 — Model gateway contract dan error taxonomy** · Lane B
  - Depends On: M0-003, M0-005
  - Parallel With: M1-001, M1-002
  - Allowed Paths: `src/model/types.rs`, `src/model/error.rs`, `src/model/mod.rs`, `src/model_gateway.rs`
  - Output: request/response netral dan error `authentication_failed`, `rate_limited`, `timeout`, `invalid_response`, `provider_unavailable`, `context_too_large`.
  - Acceptance:
    - tidak ada vendor type bocor ke domain;
    - provider body dipotong dan disanitasi pada error;
    - retryability tersedia sebagai property error.
  - Verify: unit test mapping status dan sanitasi body.

## Gelombang M1-B — Implementasi Paralel

- [x] **M1-004 — Repository provider PostgreSQL** · Lane A
  - Depends On: M1-001, M1-002
  - Parallel With: M1-005, M1-006, M1-007
  - Allowed Paths: `src/store/provider.rs`, `src/store/mod.rs`, `tests/provider_store.rs`
  - Output: CRUD provider/model dan persistence probe result.
  - Acceptance: transaksi, pagination sederhana, conflict code untuk duplicate, dan secret status dihitung dari environment saat response.
  - Verify: integration test create/read/update/delete, duplicate, dan cascade.

- [x] **M1-005 — Chat completion normal** · Lane B
  - Depends On: M1-003
  - Parallel With: M1-004, M1-007
  - Allowed Paths: `src/model/openai/chat.rs`, `src/model/openai/mod.rs`, `src/model_gateway.rs`, `tests/model_chat.rs`
  - Output: client chat normal dengan timeout dan usage normalization.
  - Acceptance: base URL `/v1` tidak hilang, missing usage ditandai estimated, response invalid ditolak aman.
  - Verify: mock tests untuk 200, 401, 429, 500, timeout, malformed JSON, dan missing usage.

- [x] **M1-006 — Streaming SSE provider** · Lane B
  - Depends On: M1-003, M1-005
  - Parallel With: M1-004, M1-007
  - Allowed Paths: `src/model/openai/stream.rs`, `src/model/openai/mod.rs`, `tests/model_stream.rs`
  - Output: parser stream SSE dan agregasi content/usage.
  - Acceptance: menangani chunk terpotong, `[DONE]`, empty delta, disconnect, dan provider error event.
  - Verify: fixture stream terfragmentasi dan disconnect test.

- [x] **M1-007 — UI provider fixtures dan komponen** · Lane C
  - Depends On: M1-001
  - Parallel With: M1-004, M1-005, M1-006
  - Allowed Paths: `web/src/lib/api/types.ts`, `web/src/lib/components/provider/**`, `web/src/lib/fixtures/provider.ts`
  - Output: form, card model, capability matrix, probe status, dan error panel memakai fixture.
  - Acceptance: API key value tidak pernah menjadi field; loading/error/empty state tersedia; keyboard dan label form valid.
  - Verify: `npm run check`, component tests bila test runner tersedia.

## Gelombang M1-C — Tool Probe dan API

- [x] **M1-008 — Tool-call normalization dan probe** · Lane B
  - Depends On: M1-005
  - Parallel With: M1-009
  - Allowed Paths: `src/model/openai/tools.rs`, `tests/model_tools.rs`
  - Output: tool definition/request/parser dan probe tool sederhana tanpa side effect.
  - Acceptance: multiple calls dapat diparse; invalid JSON argument menghasilkan typed error; tool tidak dieksekusi oleh probe.
  - Verify: mock test single, multiple, malformed argument, dan provider tanpa tools.

- [x] **M1-009 — Provider HTTP API** · Lane A
  - Depends On: M1-004, M1-005, M1-006
  - Parallel With: M1-008
  - Allowed Paths: `src/api/providers.rs`, `src/api/mod.rs`, `src/main.rs`, `tests/provider_api.rs`
  - Output: CRUD provider/model dan endpoint probe normal/streaming.
  - Acceptance: request validation, status secret tanpa nilai secret, idempotency untuk mutation, probe tersimpan.
  - Verify: API integration tests seluruh route dan error envelope.

## Gelombang M1-D — Integrasi UI dan Quality Gate

- [x] **M1-010 — Halaman Providers** · Lane C
  - Depends On: M1-007, M1-009
  - Parallel With: M1-011
  - Allowed Paths: `web/src/routes/providers/**`, `web/src/lib/api/client.ts`, `web/src/routes/+page.svelte`
  - Output: daftar, create/edit, model, dan aksi probe.
  - Acceptance: hasil normal/stream/tool terlihat per capability; secret hanya tampil `configured/missing`; error dapat dipahami.
  - Verify: `npm run check && npm run build`.

- [x] **M1-011 — Compatibility probe suite** · Lane D
  - Depends On: M1-006, M1-008, M1-009
  - Parallel With: M1-010
  - Allowed Paths: `tests/support/mock_openai.rs`, `tests/provider_compatibility.rs`, `docs/provider-compatibility.md`
  - Output: mock OpenAI-compatible server dan matriks compatibility.
  - Acceptance: normal, streaming, tool call, usage, auth, timeout, context overflow, dan rate limit tercakup.
  - Verify: `cargo test provider_compatibility`.

### Exit Gate M1

- [x] Provider dibuat dan diedit dari WebApp.
- [x] Secret dibaca dari environment dan tidak masuk DB/log/UI.
- [x] Probe normal, streaming, dan tool call tersimpan.
- [x] Capability verified terlihat di UI.
- [x] Full validation lulus.

---

# M2 — Single-Worker Vertical Slice

## Gelombang M2-A — Task dan Artifact Foundation

- [x] **M2-001 — Task domain dan state machine** · Lane A
  - Depends On: M1 exit gate
  - Parallel With: M2-002, M2-003, M2-004
  - Allowed Paths: `src/domain/task.rs`, `src/domain/state_machine.rs`, `src/api/contracts/task.rs`
  - Output: task contract, status, transition policy, attempt limit, dan validation.
  - Acceptance: Worker tidak dapat menandai `DONE`; invalid transition ditolak; acceptance criteria/allowed paths/verification wajib sebelum `READY`.
  - Verify: table-driven unit test seluruh transition valid dan invalid.

- [x] **M2-002 — Artifact store filesystem** · Lane A
  - Depends On: M1 exit gate
  - Parallel With: M2-001, M2-003, M2-004
  - Allowed Paths: `src/store/artifact.rs`, `tests/artifact_store.rs`
  - Output: write/read metadata, SHA-256, atomic rename, size limit, dan path isolation.
  - Acceptance: traversal dan symlink escape ditolak; partial write dibersihkan; DB metadata dibuat setelah file aman.
  - Verify: temp-directory integration tests.

- [x] **M2-003 — Git worktree manager** · Lane B
  - Depends On: M1 exit gate, M0-002
  - Parallel With: M2-001, M2-002, M2-004
  - Allowed Paths: `src/runner/git.rs`, `tests/git_worktree.rs`
  - Output: create worktree/branch, status, diff binary, base commit, apply patch, cleanup.
  - Acceptance: hanya repository Git valid; path shell-safe; tidak ada push; cleanup idempotent.
  - Verify: fixture repository integration test.

- [x] **M2-004 — Web task components berbasis fixture** · Lane C
  - Depends On: M1 exit gate
  - Parallel With: M2-001, M2-002, M2-003
  - Allowed Paths: `web/src/lib/components/task/**`, `web/src/lib/fixtures/task.ts`, `web/src/lib/api/types.ts`
  - Output: task contract viewer, status badge, timeline, artifact list, diff panel, dan usage card.
  - Acceptance: status tidak bergantung warna; diff dapat dibaca keyboard; empty/error/loading state tersedia.
  - Verify: `npm run check`.

## Gelombang M2-B — Context dan Tool Runner

- [x] **M2-005 — Task PostgreSQL store** · Lane A
  - Depends On: M2-001
  - Parallel With: M2-006, M2-007
  - Allowed Paths: `migrations/0004_task_runtime.sql`, `src/store/task.rs`, `src/store/event.rs`, `tests/task_store.rs`
  - Output: task CRUD, event append, agent attempt, usage, dan transactional transition.
  - Acceptance: optimistic conflict/row lock aman; event tercatat bersama transition; pagination cursor tersedia.
  - Verify: integration tests transition race dan rollback.

- [x] **M2-006 — Context builder minimum** · Lane B
  - Depends On: M2-001, M2-002
  - Parallel With: M2-005, M2-007
  - Allowed Paths: `src/context/**`, `tests/context_builder.rs`
  - Output: context dari task contract, referenced artifact, `rg`, file excerpt, dan token estimate.
  - Acceptance: obey allowed paths dan byte/token limit; binary/large file ditolak; sumber context tercatat.
  - Verify: tests relevance, truncation, path rejection, dan deterministic ordering.

- [x] **M2-007 — Tool policy dan structured tools** · Lane B
  - Depends On: M2-001, M2-003
  - Parallel With: M2-005, M2-006
  - Allowed Paths: `src/runner/policy.rs`, `src/runner/tools/**`, `tests/tool_policy.rs`
  - Output: `list_files`, `search_code`, `read_file`, `apply_patch`, `git_diff`, `git_status`, `submit_artifact`, `request_human`.
  - Acceptance: role/path/arg/timeout divalidasi; output dibatasi; shell mentah tidak tersedia; setiap call menghasilkan audit event contract.
  - Verify: tests traversal, symlink escape, denied tool, oversized output, timeout, dan valid patch.

## Gelombang M2-C — Agent Runtime dan API

- [x] **M2-008 — Process runner terisolasi** · Lane B
  - Depends On: M2-007
  - Parallel With: M2-009, M2-010
  - Allowed Paths: `src/runner/process.rs`, `src/runner/container.rs`, `tests/process_runner.rs`, `compose.yaml`
  - Output: command allowlist dalam container, timeout, CPU/RAM/process limit, env allowlist, network off default.
  - Acceptance: command arbitrary ditolak; kill process tree saat timeout; stdout/stderr dibatasi dan disimpan sebagai artifact.
  - Verify: timeout, memory/command denial, network denial, dan successful check.

- [x] **M2-009 — Worker turn loop** · Lane B
  - Depends On: M1-008, M2-003, M2-006, M2-007
  - Parallel With: M2-008, M2-010
  - Allowed Paths: `src/agent/worker.rs`, `src/agent/mod.rs`, `tests/worker_loop.rs`
  - Output: model turn, tool call, checkpoint, budget check, max turns, dan structured handoff.
  - Acceptance: side effect tercatat sebelum turn berikutnya; stop pada budget/timeout; tidak mengulang tool non-idempotent.
  - Verify: scripted fake model menyelesaikan edit sederhana dan berhenti pada limit.

- [x] **M2-010 — Task dan artifact HTTP API** · Lane A
  - Depends On: M2-002, M2-005
  - Parallel With: M2-008, M2-009
  - Allowed Paths: `src/api/tasks.rs`, `src/api/artifacts.rs`, `src/api/events.rs`, `src/api/mod.rs`, `src/main.rs`, `tests/task_api.rs`
  - Output: create/list/detail/start/cancel/retry task, artifacts, diff, dan event SSE.
  - Acceptance: mutation idempotent; SSE mendukung `Last-Event-ID`; artifact path internal tidak bocor.
  - Verify: API tests dan SSE reconnect test.

## Gelombang M2-D — Review, Verify, dan WebApp

- [x] **M2-011 — Reviewer agent** · Lane B
  - Depends On: M2-009
  - Parallel With: M2-012, M2-013
  - Allowed Paths: `src/agent/reviewer.rs`, `tests/reviewer.rs`
  - Output: reviewer menerima task contract, diff, source sekitar, dan hasil self-check.
  - Acceptance: output hanya `approved` atau `changes_requested` dengan temuan terstruktur; reviewer tidak dapat edit.
  - Verify: fake model scenarios missing requirement, path violation, dan approval.

- [x] **M2-012 — Verifier deterministik** · Lane B
  - Depends On: M2-008, M2-009
  - Parallel With: M2-011, M2-013
  - Allowed Paths: `src/agent/verifier.rs`, `tests/verifier.rs`
  - Output: menjalankan verification command allowlist dan menyimpan bukti.
  - Acceptance: status lulus berdasarkan exit code; timeout/output limit diterapkan; verifier tidak mengubah source.
  - Verify: fixture pass, fail, timeout, dan attempted mutation.

- [x] **M2-013 — Halaman task dan live events** · Lane C
  - Depends On: M2-004, M2-010
  - Parallel With: M2-011, M2-012
  - Allowed Paths: `web/src/routes/tasks/**`, `web/src/lib/api/client.ts`, `web/src/lib/realtime/**`
  - Output: create task manual, detail, start/cancel/retry, diff, artifact, test result, token, dan event realtime.
  - Acceptance: reconnect SSE mengejar event tertinggal; destructive action perlu konfirmasi; error tetap terlihat.
  - Verify: `npm run check && npm run build`.

## Gelombang M2-E — Vertical Slice Integration

- [x] **M2-014 — Single-worker orchestrator** · Lane A/B Integrator
  - Depends On: M2-010, M2-011, M2-012
  - Parallel With: M2-015
  - Allowed Paths: `src/orchestrator.rs`, `src/main.rs`, `tests/single_worker_flow.rs`
  - Output: `READY → RUNNING → SELF_CHECK → REVIEW → VERIFY → DONE/FAILED`.
  - Acceptance: crash-safe checkpoint, event tiap transition, attempt maksimal dua, usage tercatat, worktree cleanup sesuai retention.
  - Verify: end-to-end fake provider pada fixture menghasilkan patch terverifikasi.

- [x] **M2-015 — Vertical slice browser smoke** · Lane D
  - Depends On: M2-013
  - Parallel With: M2-014
  - Allowed Paths: `tests/e2e/**`, `docs/manual-test.md`, `web/package.json`, `web/package-lock.json`
  - Output: smoke test provider → task manual → progress → diff → result.
  - Acceptance: test stabil pada viewport desktop dan keyboard path utama.
  - Verify: command E2E terdokumentasi dan lulus.

### Exit Gate M2

- [x] Task manual dari WebApp selesai end-to-end.
- [x] Worker hanya membaca/edit scope task.
- [x] Patch, review, verification, event, dan usage tersedia.
- [x] Failure tidak mengubah branch dasar.
- [x] Full validation dan E2E lulus.

---

# M3 — Lead Agent dan Task DAG

## Gelombang M3-A — DAG dan Plan Contract

- [x] **M3-001 — Project/run domain dan API contract** · Lane A
  - Depends On: M2 exit gate
  - Parallel With: M3-002, M3-003, M3-004
  - Allowed Paths: `src/domain/project.rs`, `src/api/contracts/project.rs`, `src/api/contracts/plan.rs`
  - Output: project brief, run, proposed plan, approval, budget, dan risk flags.
  - Acceptance: objective dan acceptance criteria wajib; repository path canonical; budget positif.
  - Verify: boundary validation tests.

- [x] **M3-002 — DAG validation** · Lane A
  - Depends On: M2 exit gate
  - Parallel With: M3-001, M3-003, M3-004
  - Allowed Paths: `src/domain/dag.rs`, `tests/dag.rs`
  - Output: cycle detection, dependency validation, ready calculation, dan deterministic topological order.
  - Acceptance: missing/self/cyclic dependency ditolak; task blocked tidak dianggap ready.
  - Verify: property/table-driven DAG tests.

- [x] **M3-003 — Repository discovery** · Lane B
  - Depends On: M2 exit gate
  - Parallel With: M3-001, M3-002, M3-004
  - Allowed Paths: `src/context/discovery.rs`, `tests/discovery.rs`
  - Output: peta file, language/framework, entry point, test commands, config, dan scoped instruction files.
  - Acceptance: tool-first tanpa model jika cukup; file besar/binary/ignored dikecualikan; output artifact ringkas.
  - Verify: fixture polyglot dan nested instructions.

- [x] **M3-004 — Project board UI fixtures** · Lane C
  - Depends On: M2 exit gate
  - Parallel With: M3-001, M3-002, M3-003
  - Allowed Paths: `web/src/lib/components/project/**`, `web/src/lib/components/board/**`, `web/src/lib/fixtures/project.ts`
  - Output: create project/run, proposed plan, board, DAG summary, budget, dan approval UI.
  - Acceptance: dependency/status dapat dipahami tanpa drag-and-drop; approval keyboard-accessible.
  - Verify: `npm run check`.

## Gelombang M3-B — Lead dan Project API

- [x] **M3-005 — Project/run PostgreSQL store** · Lane A
  - Depends On: M3-001, M3-002
  - Parallel With: M3-006, M3-007
  - Allowed Paths: `migrations/0011_projects_and_plans.sql`, `src/store/project.rs`, `tests/project_store.rs`
  - Output: project/run/plan/dependency persistence dan transactional approval.
  - Acceptance: plan immutable setelah approval; supersede membuat version baru; budget reservation atomik.
  - Verify: integration tests version, approval race, dan rollback.

- [x] **M3-006 — Lead planner** · Lane B
  - Depends On: M3-001, M3-002, M3-003
  - Parallel With: M3-005, M3-007
  - Allowed Paths: `src/agent/lead.rs`, `src/agent/prompts/lead.md`, `tests/lead_planner.rs`
  - Output: structured proposed plan dari brief + discovery artifact.
  - Acceptance: schema tervalidasi; file scope/task dependency/budget/verification ada; invalid plan tidak disimpan sebagai approved.
  - Verify: fake model valid, malformed, cyclic, overlapping, dan over-budget plans.

- [x] **M3-007 — Project/run HTTP API** · Lane A
  - Depends On: M3-001, M3-005
  - Parallel With: M3-006
  - Allowed Paths: `src/api/projects.rs`, `src/api/runs.rs`, `src/api/mod.rs`, `src/main.rs`, `tests/project_api.rs`
  - Output: project CRUD, discovery, create run, propose/approve/reject plan, pause/resume/cancel.
  - Acceptance: approval manusia wajib; path dan repository tervalidasi; mutation idempotent.
  - Verify: API integration tests seluruh transition.

## Gelombang M3-C — Skill dan Context Policy

- [x] **M3-008 — Skill registry lazy loading** · Lane B
  - Depends On: M3-003
  - Parallel With: M3-009, M3-010
  - Allowed Paths: `src/context/skills.rs`, `skills/**`, `tests/skills.rs`
  - Output: parser Markdown frontmatter, metadata index, role/trigger match, dan content load on demand.
  - Acceptance: duplicate/invalid ID ditolak; lead hanya menerima metadata; worker maksimal dua skill default.
  - Verify: parser, matching, budget, dan reload tests.

- [x] **M3-009 — Role context policies** · Lane B
  - Depends On: M2-006, M3-003
  - Parallel With: M3-008, M3-010
  - Allowed Paths: `src/context/policy.rs`, `tests/context_policy.rs`
  - Output: context rules Lead/Architect/Worker/Reviewer/Verifier.
  - Acceptance: tiap role hanya menerima artifact/source yang dibutuhkan; over-limit memicu trim atau task split request.
  - Verify: snapshot metadata/size per role tanpa menyimpan secret.

- [x] **M3-010 — Project Run WebApp** · Lane C
  - Depends On: M3-004, M3-007
  - Parallel With: M3-008, M3-009
  - Allowed Paths: `web/src/routes/projects/**`, `web/src/routes/runs/**`, `web/src/lib/api/client.ts`, `web/src/lib/realtime/**`
  - Output: project setup, discovery, plan review, approval, board, DAG list, budget, controls.
  - Acceptance: task dependency dan blocked reason terlihat; plan bisa ditolak dengan alasan; live events reconnect.
  - Verify: `npm run check && npm run build`.

## Gelombang M3-D — Sequential Scheduler

- [x] **M3-011 — Scheduler dependency-aware satu worker** · Lane A
  - Depends On: M3-002, M3-005, M3-006, M3-009
  - Parallel With: M3-012
  - Allowed Paths: `src/scheduler/**`, `src/orchestrator.rs`, `tests/scheduler_sequential.rs`
  - Output: mengambil satu task ready berdasarkan dependency/priority dan menjalankan pipeline M2.
  - Acceptance: task blocked tidak diambil; pause/cancel dihormati; project budget direservasi sebelum run.
  - Verify: multi-task fixture selesai dalam topological order.

- [x] **M3-012 — Lead/DAG E2E scenarios** · Lane D
  - Depends On: M3-010
  - Parallel With: M3-011
  - Allowed Paths: `tests/e2e/**`, `tests/scenarios/lead/**`, `docs/manual-test.md`
  - Output: skenario plan valid, plan ditolak, cycle invalid, task blocked, dan budget kurang.
  - Acceptance: deterministic dengan fake provider; failure message terlihat di UI.
  - Verify: E2E suite terkait lulus.

### Exit Gate M3

- [x] Lead menghasilkan plan terstruktur dari repository discovery.
- [x] Manusia menyetujui plan sebelum eksekusi.
- [x] Scheduler menjalankan task sesuai dependency.
- [x] Skill dimuat lazy dan context limit dipatuhi.
- [x] Full validation dan E2E lulus.

---

# M4 — Parallel Workers dan Integrasi

## Gelombang M4-A — Claim, Lease, dan Budget

- [x] **M4-001 — Atomic task claim** · Lane A
  - Depends On: M3 exit gate
  - Parallel With: M4-002, M4-003, M4-004
  - Allowed Paths: `src/store/scheduler.rs`, `tests/task_claim.rs`
  - Output: claim dengan `FOR UPDATE SKIP LOCKED`, heartbeat, stale claim detection.
  - Acceptance: beberapa scheduler tidak mengambil task sama; ordering priority stabil; abandoned claim dapat dipulihkan.
  - Verify: concurrent PostgreSQL integration test.

- [x] **M4-002 — File lease dan overlap detection** · Lane A
  - Depends On: M3 exit gate
  - Parallel With: M4-001, M4-003, M4-004
  - Allowed Paths: `migrations/0005_file_leases.sql`, `src/store/lease.rs`, `src/domain/path_scope.rs`, `tests/file_lease.rs`
  - Output: normalized path scope, overlap detection, acquire/renew/release/expiry.
  - Acceptance: glob overlap konservatif; exact conflict ditolak atomik; stale lease dapat direbut setelah recovery.
  - Verify: concurrent lease tests dan tricky path cases.

- [x] **M4-003 — Hierarchical budget guard** · Lane A
  - Depends On: M3 exit gate
  - Parallel With: M4-001, M4-002, M4-004
  - Allowed Paths: `src/domain/budget.rs`, `src/store/budget.rs`, `tests/budget.rs`
  - Output: project/task/attempt/request reservation dan reconciliation.
  - Acceptance: warning 70%, checkpoint 85%, stop 100%, reserve 15%; concurrent spend tidak oversubscribe.
  - Verify: race, provider estimated usage, refund, dan hard-stop tests.

- [x] **M4-004 — Parallel dashboard fixtures** · Lane C
  - Depends On: M3 exit gate
  - Parallel With: M4-001, M4-002, M4-003
  - Allowed Paths: `web/src/lib/components/workers/**`, `web/src/lib/components/budget/**`, `web/src/lib/fixtures/parallel.ts`
  - Output: worker slots, lease/conflict, token gauges, queue, dan attempt display.
  - Acceptance: warning tidak hanya warna; cancel/pause status jelas; layout 2–4 workers responsif.
  - Verify: `npm run check`.

## Gelombang M4-B — Parallel Scheduler dan Integration

- [x] **M4-005 — Parallel scheduler** · Lane A
  - Depends On: M4-001, M4-002, M4-003
  - Parallel With: M4-006, M4-007
  - Allowed Paths: `src/scheduler/parallel.rs`, `src/scheduler/mod.rs`, `tests/scheduler_parallel.rs`
  - Output: 2–4 worker slots, fairness, backpressure, graceful pause/cancel.
  - Acceptance: hanya dependency-ready, lease-safe, budget-safe task berjalan; idle slot diisi; shutdown tidak mengambil task baru.
  - Verify: deterministic concurrent scenarios dengan dua scheduler instance.

- [x] **M4-006 — Integrator dan integration branch** · Lane B
  - Depends On: M2-003, M3 exit gate
  - Parallel With: M4-005, M4-007
  - Allowed Paths: `src/agent/integrator.rs`, `src/runner/integration_git.rs`, `tests/integrator.rs`
  - Output: apply approved patch berurutan, integration check, rollback, dan conflict report.
  - Acceptance: hanya patch task approved+verified; konflik semantik `NEEDS_HUMAN`; base branch tidak dimutasi; tanpa push.
  - Verify: clean apply, mechanical conflict, semantic marker, rollback, dan failed regression.

- [x] **M4-007 — Dashboard parallel live data** · Lane C
  - Depends On: M4-004 dan API/event contract M4-001..M4-003
  - Parallel With: M4-005, M4-006
  - Allowed Paths: `web/src/routes/runs/**`, `web/src/lib/components/workers/**`, `web/src/lib/components/budget/**`, `web/src/lib/api/**`
  - Output: queue, active workers, lease, attempts, budget, conflict, pause/resume/cancel realtime.
  - Acceptance: refresh/reconnect tidak menggandakan event; controls menunjukkan pending result; blocked reason terlihat.
  - Verify: `npm run check && npm run build`.

## Gelombang M4-C — Recovery dan Multi-Agent E2E

- [x] **M4-008 — Startup recovery** · Lane A
  - Depends On: M4-005, M4-006
  - Parallel With: M4-009, M4-010
  - Allowed Paths: `src/recovery.rs`, `src/main.rs`, `tests/recovery.rs`
  - Output: detect stale heartbeat, inspect checkpoint/worktree/tool side effect, resume atau attempt baru.
  - Acceptance: tool side effect tidak diulang tanpa idempotency proof; lease dilepas/renew tepat; event recovery tercatat.
  - Verify: kill/restart tests pada setiap state penting.

- [x] **M4-009 — Approval dan conflict UI** · Lane C
  - Depends On: M4-006, M4-007
  - Parallel With: M4-008, M4-010
  - Allowed Paths: `web/src/routes/approvals/**`, `web/src/lib/components/approval/**`, `web/src/lib/api/**`
  - Output: queue approval, diff/conflict context, approve/reject dengan alasan.
  - Acceptance: destructive/risky action memerlukan explicit confirmation; stale approval ditolak; audit actor terlihat.
  - Verify: `npm run check && npm run build`.

- [x] **M4-010 — Parallel E2E suite** · Lane D
  - Depends On: M4-005, M4-006, M4-007
  - Parallel With: M4-008, M4-009
  - Allowed Paths: `tests/e2e/**`, `tests/scenarios/parallel/**`
  - Output: independent tasks, overlap held, retry, conflict, budget stop, dan integration regression.
  - Acceptance: 2–4 worker scenario repeatable; no duplicate claim/edit; branch dasar utuh saat gagal.
  - Verify: parallel E2E suite dijalankan tiga kali tanpa flaky failure.

### Exit Gate M4

- [x] Dua sampai empat task independen berjalan paralel.
- [x] Task overlap tidak berjalan bersama.
- [x] Patch lolos review+verify sebelum integrasi.
- [x] Conflict memerlukan manusia; branch dasar tetap aman.
- [x] Restart memulihkan run tanpa duplicate side effect.
- [x] Full validation dan E2E lulus.

---

# M5 — Hardening dan MVP Release

## Gelombang M5-A — Security, Retention, dan Observability

- [x] **M5-001 — Security hardening** · Lane B/D
  - Depends On: M4 exit gate
  - Parallel With: M5-002, M5-003, M5-004
  - Allowed Paths: `src/security/**`, `src/runner/**`, `tests/security.rs`, `docs/security.md`
  - Output: secret redaction, symlink/path audit, command/env allowlist audit, request/body limits, CORS config.
  - Acceptance: test traversal, symlink, prompt/tool secret leakage, command injection, oversized payload, network denial.
  - Verify: security test suite dan manual threat checklist.

- [x] **M5-002 — Retention dan cleanup** · Lane A
  - Depends On: M4 exit gate
  - Parallel With: M5-001, M5-003, M5-004
  - Allowed Paths: `src/retention.rs`, `src/store/artifact.rs`, `src/runner/git.rs`, `tests/retention.rs`
  - Output: cleanup worktree/artifact/context debug berdasarkan policy, dry-run, dan audit event.
  - Acceptance: active/referenced artifact tidak terhapus; failure aman untuk retry; DB/file consistency dipertahankan.
  - Verify: time-controlled cleanup tests.

- [x] **M5-003 — Metrics dan operational health** · Lane A
  - Depends On: M4 exit gate
  - Parallel With: M5-001, M5-002, M5-004
  - Allowed Paths: `src/observability.rs`, `src/api/health.rs`, `src/main.rs`, `tests/health.rs`
  - Output: liveness/readiness, structured metrics endpoint, queue/worker/provider/DB health.
  - Acceptance: readiness gagal jika DB/migration invalid; liveness tidak tergantung provider; label metrics bounded.
  - Verify: health degradation tests.

- [x] **M5-004 — Web accessibility dan responsive audit** · Lane C
  - Depends On: M4 exit gate
  - Parallel With: M5-001, M5-002, M5-003
  - Allowed Paths: `web/src/**`, `tests/e2e/accessibility.*`
  - Output: keyboard, focus, semantic labels, contrast, reduced motion, mobile layout.
  - Acceptance: alur provider/project/task/approval dapat selesai tanpa mouse; status tidak hanya warna.
  - Verify: `npm run check`, accessibility automation, dan keyboard smoke.

## Gelombang M5-B — Fallback, Backup, dan Packaging

- [x] **M5-005 — Provider retry dan fallback aman** · Lane B
  - Depends On: M5-001
  - Parallel With: M5-006, M5-007, M5-008
  - Allowed Paths: `src/model/retry.rs`, `src/model/router.rs`, `tests/model_fallback.rs`
  - Output: retry timeout/rate-limit/unavailable dan fallback berdasarkan capability/class.
  - Acceptance: auth/invalid/context/budget error tidak retry; side effect checkpoint diperiksa; exponential backoff bounded.
  - Verify: fake clock tests untuk matrix retry/fallback.

- [x] **M5-006 — Backup dan restore metadata** · Lane A/D
  - Depends On: M5-002
  - Parallel With: M5-005, M5-007, M5-008
  - Allowed Paths: `scripts/backup.sh`, `scripts/restore.sh`, `docs/backup.md`, `tests/backup/**`
  - Output: PostgreSQL dump + artifact manifest dan restore procedure.
  - Acceptance: secret tidak masuk archive; checksum artifact diverifikasi; restore ke environment kosong berhasil.
  - Verify: backup/restore drill pada fixture run.

- [ ] **M5-007 — Production container dan static WebApp serving** · Lane D
  - Depends On: M5-003, M5-004
  - Parallel With: M5-005, M5-006, M5-008
  - Allowed Paths: `Dockerfile`, `compose.yaml`, `.dockerignore`, `src/main.rs`, `Cargo.toml`, `Cargo.lock`, `web/**`, `docs/deployment.md`
  - Output: multi-stage image, non-root runtime, Axum melayani WebApp static, healthcheck.
  - Acceptance: image tidak memuat source secret/node_modules/target; read-only root bila mungkin; localhost default terdokumentasi.
  - Verify: clean image build dan browser smoke dari Compose.

- [x] **M5-008 — Operations UI** · Lane C
  - Depends On: M5-003
  - Parallel With: M5-005, M5-006, M5-007
  - Allowed Paths: `web/src/routes/settings/**`, `web/src/routes/operations/**`, `web/src/lib/components/metrics/**`, `web/src/lib/api/**`
  - Output: health, usage/cost, retention status, provider readiness, dan config non-secret.
  - Acceptance: secret value tidak tampil; estimated usage diberi label; degraded component jelas.
  - Verify: `npm run check && npm run build`.

## Gelombang M5-C — Release Validation

- [ ] **M5-009 — Full deterministic E2E matrix** · Lane D
  - Depends On: M5-005, M5-007, M5-008
  - Parallel With: M5-010, M5-011
  - Allowed Paths: `tests/e2e/**`, `tests/scenarios/**`, `docs/test-matrix.md`
  - Output: seluruh skenario wajib rancangan dalam satu command.
  - Acceptance: backend/frontend parallel, conflict, retry, timeout, budget, approval, restart, provider fallback, test failure semuanya tercakup.
  - Verify: suite clean environment lulus tiga kali.

- [ ] **M5-010 — Benchmark terhadap baseline** · Lane D
  - Depends On: M0-004, M5-009
  - Parallel With: M5-011
  - Allowed Paths: `docs/benchmark.md`, `tests/baseline/**`
  - Output: perbandingan single-agent vs MVP pada lima task tetap.
  - Acceptance: laporan success rate, token/task, duration, retry, conflicts, test pass, intervention, cost; kegagalan target dijelaskan.
  - Verify: data mentah dirujuk dan command repeatable.

- [ ] **M5-011 — Dokumentasi operator dan user** · Lane D
  - Depends On: M5-006, M5-007, M5-008
  - Parallel With: M5-009, M5-010
  - Allowed Paths: `README.md`, `docs/operations.md`, `docs/user-guide.md`, `docs/troubleshooting.md`, `.env.example`, `config/example.toml`
  - Output: setup, provider, project run, approval, recovery, backup, upgrade, dan troubleshooting.
  - Acceptance: fresh install dapat dilakukan hanya dari docs; semua env/config terdokumentasi; batas keamanan dinyatakan jujur.
  - Verify: fresh-install walkthrough.

- [ ] **M5-012 — MVP release gate** · Integrator
  - Depends On: M5-009, M5-010, M5-011
  - Parallel With: —
  - Allowed Paths: seluruh repository hanya untuk fix blocker terverifikasi dan release notes
  - Output: release candidate, daftar known limitations, dan hasil seluruh gate.
  - Acceptance:
    - median input token turun minimal 50% dari baseline atau release ditahan;
    - minimal 80% task fixture selesai tanpa intervensi manual;
    - tidak ada edit di luar allowed paths;
    - seluruh task `DONE` memiliki review, verification, patch, event, dan usage;
    - restart tidak kehilangan status;
    - 2–4 worker dapat dipantau/dihentikan;
    - branch dasar aman pada semua failure scenario.
  - Verify: seluruh Gerbang Validasi, E2E matrix, security suite, backup/restore drill, dan benchmark.

### Exit Gate M5 / MVP Selesai

- [ ] Semua acceptance M5-012 terpenuhi.
- [ ] Tidak ada blocker severity tinggi.
- [ ] Known limitations terdokumentasi.
- [ ] Deployment lokal/self-hosted dapat diulang dari environment kosong.

---

# 7. Rencana Penugasan 2–4 Agent

Gunakan pola berikut per gelombang. Jangan menjalankan task dari gelombang berikut sebelum dependency selesai.

## Jika 2 Agent

| Agent | Fokus |
|---|---|
| Agent 1 | Lane A lalu integrasi backend |
| Agent 2 | Lane B; saat menunggu dependency, ambil Lane D atau C fixture |

UI dikerjakan setelah kontrak API stabil. Agent 1 menjadi Integrator setiap akhir milestone.

## Jika 3 Agent

| Agent | Fokus |
|---|---|
| Agent 1 | Lane A — domain/store/API/scheduler |
| Agent 2 | Lane B — model/context/runner/agent |
| Agent 3 | Lane C + D — WebApp, fixture, E2E, docs |

Agent 3 dapat membuat UI dari fixture sambil backend berjalan.

## Jika 4 Agent

| Agent | Fokus |
|---|---|
| Agent 1 | Lane A — Control Plane |
| Agent 2 | Lane B — Execution Plane |
| Agent 3 | Lane C — WebApp |
| Agent 4 | Lane D — QA/Platform/Integrator |

Agent 4 tidak mengubah implementasi milik agent lain saat task aktif. Agent 4 menulis contract tests, menjalankan gate, dan mengintegrasikan setelah handoff.

## Contoh Gelombang Pertama

```text
Agent 1: M0-003 Konfigurasi runtime typed
Agent 2: M0-002 Repository fixture E2E
Agent 3: M0-001 Governance repository
Agent 4: idle/review; lalu M0-004 setelah M0-002 selesai
```

# 8. Catatan Aktif

| Task | Agent | Started At | Branch/Worktree | Status/Blocker |
|---|---|---|---|---|
| — | — | — | — | — |

# 9. Keputusan dan Blocker

Catat keputusan yang memengaruhi lebih dari satu task. Jangan menyimpan diskusi panjang.

| Date | Task | Decision/Blocker | Owner | Follow-up |
|---|---|---|---|---|
| 2026-09-19 | PROJECT | Rust + Axum, SvelteKit, PostgreSQL 16, OpenAI-compatible API, container runner, approval setiap plan, tanpa Git push | Human | Implement M0–M5 |
| 2026-10-06 | M3-007B | Tambah endpoint baca di luar daftar task: `GET /projects/:id/runs`, `GET /runs/:id/plans`, `budget` pada `GET /runs/:id`, filter `GET /tasks?project_run_id=`. Filter ini menggantikan `GET /runs/:id/tasks` dari rancangan; SSE level run (`/runs/:id/events[/stream]`) belum ada dan UI memakai polling | Human | SSE run bila polling tidak cukup; RANCANGAN.md sudah disesuaikan |
| 2026-10-06 | M3-006B | Tambah `POST /runs/:id/lead-plan`: ID plan/task dari model diberi prefix run dan versi dipaksa urut database (ID task unik global) | Human | Usage panggilan Lead belum masuk `model_usage` dan endpoint belum di-rate-limit; selesaikan di M4-003 / M5-001 |
| 2026-10-06 | M3-004, M3-010 | Form project/run mengisi UUID otomatis; `PlanReview` menampilkan reservasi Σ(input+output) sama dengan backend (bukan × attempts) | Human | Lead tetap menolak plan bila Σ × attempts > budget (lebih ketat dari reservasi approval) |
| 2026-10-06 | M4-002, M4-003 | Penyimpangan dari Allowed Paths yang dibenarkan: migration `0013_file_leases.sql` (nomor 0005 di tracker sudah terpakai) dan `0014_budget_reservations.sql` (M4-003 tidak mencantumkan `migrations/`, tetapi reservasi per request harus tahan lama dan terlihat semua scheduler) | Human | Migration tetap append-only |
| 2026-10-06 | M4-005 | `ParallelScheduler` mendelegasikan eksekusi ke trait `SlotRunner` (karena `Orchestrator::dispatch_claimed` memakai `&mut self`). Perubahan di luar Allowed Paths: `src/store/scheduler.rs` (claim per-task, daftar kandidat, hitungan aktif) dan satu baris `src/orchestrator.rs` (`pub mod scheduler`) | Human | Buat adaptor `SlotRunner` produksi + wiring `main.rs`; saat ini produksi masih `SequentialScheduler` |
| 2026-10-06 | M4-006 | Integrator hanya MENGUSULKAN transisi (Done, atau Conflict lalu NeedsHuman); belum dipanggil orchestrator dan `IntegrationCheck` produksi (build/test di container) belum ada. Regresi pemeriksaan integrasi diperlakukan sebagai konflik semantik (NEEDS_HUMAN) | Human | Task integrasi: ambil patch + hasil review/verify dari DB, terapkan transisi, simpan laporan konflik sebagai artifact |
| 2026-10-06 | M4-007B, M4-009B | Endpoint baca tambahan di luar daftar task: `GET /runs/:id/scheduler` dan `GET /approvals`; `retry`/`cancel` menerima `actor_id`/`reason` dan dijalankan sebagai `Actor::Human` bila sah (sebelumnya `NEEDS_HUMAN -> READY` tidak mungkin lewat API) | Human | Identitas masih nama yang diketik, belum autentikasi |
| 2026-10-06 | M4-008 | `recovery::recover` dipanggil dari `main.rs` sebelum recovery integrasi orchestrator; memperbaiki crash tepat setelah claim (ASSIGNED tanpa worktree sebelumnya membuat startup gagal) dan diserialkan dengan advisory lock sesi. Attempt baru dipulihkan setelah `stale_after_seconds` | Human | `ParallelScheduler::recover` belum memakai inspeksi worktree |
| 2026-10-06 | M4-005B | `OrchestratorRunner` (adaptor `SlotRunner` produksi) menjalankan pipeline untuk task yang diklaim `ParallelScheduler`; `ClaimedTask` membawa `owner`; `WorkerClock` kini `Send + Sync`; `main.rs` memakai `ParallelScheduler` (provider dicari setelah model terdaftar). Attempt yang dimulai manual lewat API tetap dilayani `Orchestrator::run` dan tidak dihitung dalam batas slot | Human | Budget guard belum dipanggil dari worker |
| 2026-10-06 | M4-006B | Integrator dipakai pipeline: patch task ditumpuk di cabang integrasi per run (`noctis-integration-<run>`), pemeriksaan integrasi = verifikasi task diulang di cabang itu, konflik berakhir CONFLICT lalu NEEDS_HUMAN dengan artifact `conflict_report`, task dimulai dari head cabang integrasi, recovery operasi cabang run (commit `integrate <task>` ada = DONE, selain itu NEEDS_HUMAN). `Verifier::with_artifact_scope` membedakan artifact pemeriksaan integrasi | Human | Laporan konflik belum tampil di UI |
| 2026-10-06 | M4-010 | E2E paralel menemukan bug: retry setelah `changes_requested` selalu gagal `orchestrator.git` karena worktree attempt lama menempati path task. Diperbaiki di `src/orchestrator.rs` (`release_previous_worktree`, di luar Allowed Paths task) | Human | Skenario retry E2E menjadi regression test |
| 2026-10-06 | M4 | Exit Gate M4 terpenuhi: scheduler paralel dan Integrator terhubung ke jalur produksi (M4-005B, M4-006B) dan diuji E2E 2–4 worker (M4-010) | Human | Lanjut M5 |
| 2026-10-07 | M5-003 | Penyimpangan dari Allowed Paths yang dibenarkan: `src/api/mod.rs` dan `src/lib.rs` (daftar modul). Handler `/api/v1/health` dipindah dari `main.rs` ke `src/api/health.rs` dan tetap kompatibel. Metrik ditulis sebagai teks Prometheus tanpa dependensi baru; log belum JSON (butuh fitur `json` di Cargo.toml) | Human | Log JSON stdout (aturan umum #2) perlu task/izin mengubah Cargo.toml
| 2026-10-07 | M5-002 | Penyimpangan dari Allowed Paths yang dibenarkan: migration `0015_retention_actions.sql` (jejak audit; tabel `events` mewajibkan `project_run_id` sedangkan artifact yatim tidak punya run), `src/lib.rs`, `src/main.rs` (loop retensi tiap jam), dan test `tests/artifact_store.rs`. Artifact yang direferensikan (tabel, tool reservation, payload event) tidak pernah dihapus; branch integrasi run dipertahankan, hanya worktree-nya yang dibersihkan. "Context debug" tidak ada di kode sehingga N/A | Human | Kebijakan kedaluwarsa untuk artifact yang direferensikan butuh keputusan terpisah (gerbang M5-012 mewajibkan bukti task DONE)
| 2026-10-07 | LOG | Log aplikasi kini JSON ke stdout dengan `request_id` di setiap log request (span `http`); fitur `json` ditambahkan ke `tracing-subscriber` (+`tracing-serde`, izin eksplisit pemilik). `X-Request-Id` klien diterima bila UUID kanonis, selain itu diganti. Query/header/body tidak pernah dicatat | Human | `trace_id` tidak ada (monolith tanpa OpenTelemetry)
| 2026-10-07 | M5-001 | Penyimpangan dari Allowed Paths yang dibenarkan: redaksi dipasang di `src/model/openai/{chat,stream,tools}.rs` (satu choke point prompt ke provider), `src/api/mod.rs`, `src/config.rs`, `src/main.rs`, `src/lib.rs` (`extern crate self as ai_team` karena test meng-include sumber klien), `config/example.toml`, dan `tests/process_runner.rs`. Kunci baru `server.cors_allowed_origins` (default `http://127.0.0.1:5173`; wildcard ditolak). Redaksi mengurangi, bukan menjamin; tidak ada autentikasi API (dinyatakan di `docs/security.md`) | Human | Autentikasi/otorisasi API, rate limiting per pengguna, `cargo audit`/`npm audit` otomatis
| 2026-10-07 | M5-004 | Audit a11y ditulis sendiri di Playwright (tanpa axe-core: butuh dependensi baru di luar Allowed Paths). Tambahan di luar daftar: `web/src/app.css` dan `web/src/routes/+layout.svelte` (CSS global tanpa mengubah DOM). Tidak ada navbar/skip-link karena `navigation.spec` mengunci urutan Tab; tautan Approvals ditambahkan di beranda | Human | Audit manual dengan pembaca layar, zoom 200%/400%, dan mode kontras tinggi belum dilakukan
| 2026-10-07 | M5-005 | Penyimpangan dari Allowed Paths yang dibenarkan: `src/model_guard.rs` (guard PostgreSQL; tidak bisa di `src/model/` karena test meng-include modul itu sebagai crate mandiri), `src/model/error.rs` (`BudgetExceeded`), `src/model/mod.rs`, `src/agent/{worker,reviewer}.rs`, `src/orchestrator.rs`, `src/store/task.rs` (`pool()`), `src/api/providers.rs`, `src/lib.rs`. Memperbaiki `call_model` yang mengulang semua error tanpa jeda. Gerbang budget kini aktif di jalur model worker/reviewer. **Fallback lintas provider TIDAK diaktifkan di produksi** (hanya satu kandidat) karena mengirim kode repository ke provider lain; butuh konfigurasi eksplisit | Human | Config daftar `fallback_models`, `Retry-After`, router untuk Lead Agent
| 2026-10-07 | M5-006 | Penyimpangan dari Allowed Paths yang dibenarkan: `scripts/lib-backup.sh` (fungsi bersama kedua skrip). Skrip memakai `docker exec` ke container Postgres karena host tidak punya `pg_dump`/`psql`. Drill berupa test Rust `tests/backup/main.rs` (dilewati bila tidak ada container yang menerbitkan port 55432). Repository Git proyek dan branch integrasi tidak ikut backup; arsip tidak dienkripsi | Human | Backup terjadwal/rotasi, enkripsi, backup repository Git, `pg_dump -Fc` untuk database besar
| 2026-10-07 | M5-008 | Penyimpangan dari Allowed Paths yang dibenarkan: endpoint backend baru `src/api/operations.rs` (+ `src/api/mod.rs`, `src/main.rs`) karena data retensi, kesiapan provider, dan konfigurasi belum punya API; `tests/operations_api.rs`, `tests/e2e/operations.spec.cjs`, dua rute di `tests/e2e/accessibility.spec.cjs`, dan tautan di `web/src/routes/+page.svelte`. Halaman read-only karena API belum punya autentikasi. Biaya tampil hanya bila provider mengisi `cost_micros` (mata uang tidak dicatat) | Human | Aksi dari UI (jalankan retensi/backup) menunggu autentikasi; hasil backup tidak tercatat di database

# 10. Progress Log

Tambahkan satu baris saat task selesai atau diblokir.

| Date | Task | Result | Verification | Notes |
|---|---|---|---|---|
| 2026-09-19 | BOOT-001..004 | Foundation complete | `cargo test`, `npm run check`, `npm run build`, DB health | Provider probe masih minimum |
| 2026-09-19 | M0-001..005 | Milestone M0 complete; integration commit `eba44d1` | `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test`; `npm run check`; `npm run build`; `docker compose config --quiet` | Semua full validation lulus |
| 2026-09-19 | M1-A | Cross-contract provider foundation lulus; test commit `6651bb1` | `cargo test --test provider_contract -- --test-threads=1 --nocapture`; `cargo fmt --check`; `cargo test`; `cargo clippy --all-targets --all-features -- -D warnings`; `git diff --check` | PostgreSQL aktif dari instance sehat; roundtrip domain, capability, probe, ID, dan bigint lossless |
| 2026-09-20 | M1-004, M1-005, M1-007 | Provider store, non-streaming chat client, dan UI provider foundation terintegrasi | `cargo test --test provider_store -- --test-threads=1 --nocapture`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `npm run check`; `npm run build` | M1-006 tetap terbuka; milestone M1 belum selesai |
| 2026-09-20 | M1-006, M1-008 | Streaming SSE dan tool-call probe terintegrasi; tool client ter-wire ke production crate | `cargo test --test model_stream --test model_tools`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo fmt --check`; `git diff --check` | `M1-009` kini dapat dimulai; milestone M1 belum selesai |
| 2026-09-20 | M1-009, M1-010, M1-011A | Provider HTTP API dan halaman Providers terintegrasi; compatibility suite model-level tersedia | `cargo test --test provider_api -- --test-threads=1 --nocapture`; `cargo test --test provider_compatibility -- --nocapture`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `npm run check`; `npm run build` | Kontrak list memakai page envelope; M1-011 menunggu verifikasi API/persistence |
| 2026-09-20 | M1-011, Exit Gate M1 | Compatibility API/persistence, SSRF guard, CORS preflight, dan error envelope final lulus | `cargo test --test provider_api -- --test-threads=1 --nocapture`; `cargo test --test provider_compatibility -- --test-threads=1 --nocapture`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `npm run check`; `npm run build` | Milestone M1 selesai; private provider wajib masuk `NOCTIS_PROVIDER_HOST_ALLOWLIST` |
| 2026-09-20 | M2-001, M2-002 | Task state machine dan filesystem artifact store terintegrasi ke production crate | `cargo test --test artifact_store -- --nocapture`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo fmt --check`; `git diff --check` | Migration task runtime memakai nomor `0004` karena `0003` sudah dipakai idempotency |
| 2026-09-20 | M2-003, M2-004 | Git worktree manager aman dan komponen task fixture terintegrasi | `cargo test --test git_worktree -- --nocapture`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `npm run check`; `npm run build`; `git diff --check` | M2-005 dan M2-006 menunggu revisi review |
| 2026-09-20 | M2-006, M2-007 | Context builder bounded dan structured tools terintegrasi ke production crate | `cargo test --test context_builder --test tool_policy -- --nocapture`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo fmt --check`; `git diff --check` | Timeout filesystem cooperative; M2-005 menunggu dua revisi review |
| 2026-09-20 | M2-005, M2-008 | Canonical task store dan Docker process runner terintegrasi ke production crate | `cargo test --test task_store -- --test-threads=1 --nocapture`; `cargo test --test process_runner -- --test-threads=1 --nocapture`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `docker compose config --quiet`; `git diff --check` | Legacy `FAILED_FINAL` dimigrasikan ke `FAILED`; verification runner tidak memiliki host fallback |
| 2026-09-20 | M2-009 | Worker turn loop terintegrasi ke production crate dengan deadline pasca-model dan pasca-checkpoint | `cargo test --test worker_loop -- --nocapture`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo fmt --check`; `git diff --check` | Deadline cooperative; hasil operasi yang kembali terlambat ditolak |
| 2026-09-20 | M2-012 | Verifier deterministik terintegrasi ke production crate | `cargo test --test verifier -- --nocapture`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo fmt --check`; `git diff --check` | Verdict berdasarkan exit code; mutasi source menghasilkan policy failure |
| 2026-09-20 | M2-010 | Task/artifact HTTP API terintegrasi dengan versioned delete dan live SSE bounded | `cargo test --test task_store --test task_api -- --test-threads=1 --nocapture`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo fmt --check`; `git diff --check` | SSE mendukung heartbeat dan reconnect `Last-Event-ID` tanpa duplikat; CORS mengekspos `x-request-id` |
| 2026-09-20 | M2-011 | Reviewer read-only terintegrasi dengan bounded mutation snapshot | `cargo test --test reviewer -- --test-threads=1 --nocapture`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo fmt --check`; `git diff --check` | Test memakai production crate; tracked dan untracked mutation menghasilkan typed policy failure |
| 2026-09-20 | M2-013 | Halaman task dan live events terintegrasi dengan lifecycle SSE aman | `npm run check`; `npm run build`; `git diff --check` | Native `EventSource` mengejar event tertinggal dan ditutup saat navigasi; destructive cancel wajib konfirmasi |
| 2026-09-21 | M2-014, M2-015, Exit Gate M2 | Orchestrator crash-safe dan vertical browser smoke production terintegrasi | `cargo fmt --check`; `cargo test -- --test-threads=1`; `cargo clippy --all-targets --all-features -- -D warnings`; `npm run check`; `npm run build`; `npm run e2e`; `git diff --check` | E2E 2 passed tanpa skip; integration checkpoint, recovery, retention lease, SSE reconnect, artifact, diff, verification, dan usage terverifikasi |
| 2026-09-27 | M3-001, M3-002, M3-003 | Kontrak project/plan, validasi DAG, dan repository discovery terintegrasi (`c2edf29`, `6e48380`, tracker `2416e58`) | Bukti per-task di commit tersebut; suite penuh lulus pada 2026-10-06 (lihat baris Exit Gate M3) | Command per task tidak dicatat saat integrasi |
| 2026-09-27 | M3-005, M3-006, M3-009 | Store project/plan, Lead planner terstruktur, dan policy context per role terintegrasi (`262d2dd`, `a8adc26`, `3fdcdfc`, tracker `2407293`) | Bukti per-task di commit tersebut; suite penuh lulus pada 2026-10-06 | Plan immutable setelah approval; budget direservasi atomik |
| 2026-09-28 | M3-007, M3-008, M3-011 | HTTP API project/run, skill registry lazy loading, dan scheduler sequential terintegrasi (`0d1807d`, `7bb05ab`, `b7f4079`, `5671182`, `3c8fb9d`, `7c82261`, tracker `3ed9eb4`) | Bukti per-task di commit tersebut; suite penuh lulus pada 2026-10-06 | Scheduler hanya mengambil task READY dengan dependency DONE |
| 2026-10-05 | M3-011 | Scheduler tidak lagi error saat model default belum terdaftar (`1d234c8`) | `cargo test --test scheduler_sequential` (dua regression test) | Penyebab error kini tercatat lewat `tracing` |
| 2026-10-06 | M3-004 | Komponen project board berbasis fixture terintegrasi (`a796db0`) | `npm run check`; `node web/src/lib/components/board/graph.test.mjs` | Render SSR komponen diperiksa lewat route sementara |
| 2026-10-06 | M3-007B | Endpoint baca run/plan/budget dan filter task per run (`76d2838`) | `cargo test` penuh 310 passed; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo fmt --check` | Prasyarat M3-010; bentuk endpoint berbeda dari rancangan, lihat Keputusan |
| 2026-10-06 | M3-010 | Halaman Projects dan Runs terintegrasi (`4518990`) | `npm run check`; `npm run build`; `node web/src/lib/realtime/run-poller.test.mjs`; `npm run e2e` (2 passed) | Update run lewat polling 3 detik dengan backoff |
| 2026-10-06 | M3-006B, M3-010 follow-up | Route Lead Agent, UUID otomatis, angka reservasi, link beranda, tombol Ask Lead (`2d17276`..`c64fd4c`) | `cargo test --test lead_api` (3 passed); `cargo test` penuh 313 passed; `npm run check`; `npm run build`; `npm run e2e` | Pengecekan mutasi pada `namespace_plan` membuat 2 test gagal |
| 2026-10-06 | M3-012 | E2E skenario Lead/DAG: plan valid, reject lalu re-plan, siklus, over-budget Lead, budget kurang saat approve (`3081dde`) | `NOCTIS_E2E_API_KEY=… npm run e2e` dijalankan 3 kali berturut-turut: 7 passed tiap kali | Pengecekan mutasi pada skenario `cycle` membuat test gagal |
| 2026-10-06 | Exit Gate M3 | Milestone M3 selesai | `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test -- --test-threads=1` (313 passed, 0 failed); `npm run check`; `npm run build`; `docker compose config --quiet`; `git diff --check`; `npm run e2e` (7 passed) | Plan hanya masuk lewat Lead atau API; belum ada dependensi M4 |
| 2026-10-06 | M4-001 | Claim task atomik `SKIP LOCKED` terintegrasi (`5544eaf`; perluasan di M4-005) | `cargo test --test task_claim -- --test-threads=1` (8 passed); `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings` | Mutasi pada `SKIP LOCKED` dan kunci run ditangkap test; test deterministik "kandidat teratas dikunci → dilewati" |
| 2026-10-06 | M4-002 | `PathScope` dan file lease atomik per run terintegrasi (`55b3033`) | `cargo test --test file_lease` (9 passed); `cargo test --lib path_scope` (3 passed); suite penuh `cargo test -- --test-threads=1` | Overlap glob konservatif; mutasi pada advisory lock ditangkap |
| 2026-10-06 | M4-003 | Budget guard hierarkis dengan reservasi token terintegrasi (`edc40ef`) | `cargo test --test budget` (8 passed); `cargo test --lib budget` (7 passed); suite penuh 346 passed | Belum dipanggil worker/orchestrator; hindari mengaktifkannya bersamaan dengan `record_usage_once` agar usage tak terhitung dua kali |
| 2026-10-06 | M4-004 | Komponen dashboard paralel berbasis fixture terintegrasi (`b348eee`) | `npm run check`; `npm run build`; `node web/src/lib/components/workers/helpers.test.mjs`; render Chromium 360/768/1440 px tanpa overflow horizontal | Route sementara untuk render dihapus |
| 2026-10-06 | M4-005 | Scheduler paralel 1–4 slot terintegrasi (`ddafaad`) | `cargo test --test scheduler_parallel -- --test-threads=1` (12 passed, diulang 15×); `cargo test --lib pick`; suite penuh 360 passed | Diuji dengan `SlotRunner` palsu; belum tersambung ke produksi (lihat Keputusan) |
| 2026-10-06 | M4-006 | Integrator dan cabang integrasi per run terintegrasi (`1208ac4`) | `cargo test --test integrator` (12 passed, diulang 10×); suite penuh 372 passed | Branch dasar tak dimutasi dan tanpa push (diuji dengan remote bare); belum dipanggil orchestrator |
| 2026-10-06 | M4-007 | Dashboard paralel live di halaman run, termasuk backend baca M4-007B (`ef9c485`, `3036662`) | `cargo test --test scheduler_view` (3 passed); suite penuh 375 passed; `npm run check`; `npm run build`; `npm run e2e` (7 passed, 3×) | Polling 3 detik; hasil pause/cancel tetap terlihat sampai slot kosong; satu regresi locator E2E M3-012 diperbaiki |
| 2026-10-06 | M4-008 | Recovery startup terintegrasi (`e3599cb`) | `cargo test --test recovery -- --test-threads=1` (9 passed, 20× stabil); suite penuh 385 passed | Memperbaiki startup gagal setelah crash tepat setelah claim; balapan pemulih diserialkan dengan advisory lock |
| 2026-10-06 | M4-009 | Halaman Approvals dengan konfirmasi eksplisit dan audit, termasuk backend M4-009B (`d97c50c`, `c4658af`) | `cargo test --test approvals_api` (3 passed); suite penuh 388 passed; `npm run check`; `npm run build`; `node web/src/lib/components/approval/helpers.test.mjs`; `npm run e2e` (7 passed, 2×) | Keputusan basi (409) ditolak dan dijelaskan; identitas belum diautentikasi |
| 2026-10-06 | M4-005B | Adaptor `SlotRunner` produksi terintegrasi (`f443e4b`) | `cargo test --test single_worker_flow -- --test-threads=1` (8 passed); suite penuh; clippy; `npm run e2e` (7 passed) | `ParallelScheduler` kini menjalankan pipeline asli di `main.rs`; berhenti kooperatif via `SlotControl` |
| 2026-10-06 | M4-006B | Integrator tersambung ke pipeline produksi (`89865ee`) | `cargo test --test single_worker_flow --test scheduler_sequential -- --test-threads=1`; suite penuh; clippy; `npm run e2e` (7 passed); mutation check recovery/discard/rebase | Cabang integrasi per run, konflik → NEEDS_HUMAN + `conflict_report`; task mulai dari head cabang integrasi |
| 2026-10-07 | M4-010, Exit Gate M4 | E2E paralel 2–4 worker lulus; Exit Gate M4 terpenuhi | `npm run e2e` (13 passed, 3× berturut-turut tanpa flaky); `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test -- --test-threads=1` (43 binary lulus); mutation check budget stop | Menemukan dan memperbaiki bug retry (worktree attempt lama); skenario: independen 2 dan 4 worker, overlap ditahan, retry, regresi integrasi, budget stop |
| 2026-10-07 | M5-003 | Health, readiness, dan metrik operasional terintegrasi | `cargo test --test health -- --test-threads=1` (5 passed); `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test -- --test-threads=1`; `npm run e2e` (13 passed); mutation check readiness dan liveness | Readiness hanya bergantung pada DB + migration; worker/provider `degraded`; label metrik dari himpunan tetap |
| 2026-10-07 | M5-002 | Retensi artifact yatim dan worktree cabang integrasi terintegrasi | `cargo test --test retention -- --test-threads=1` (5 passed); `cargo test --test artifact_store`; `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test -- --test-threads=1`; `npm run e2e` (13 passed); mutation check referensi/umur/branch | Dry-run dan audit `retention_actions`; kegagalan hapus aman diulang |
| 2026-10-07 | LOG | Log JSON terstruktur dan korelasi `request_id` terintegrasi | `cargo test --test logging` (2 passed); `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test -- --test-threads=1`; `npm run e2e` (13 passed); mutation check span/header | Aturan umum #2 terpenuhi untuk log aplikasi |
| 2026-10-07 | M5-001 | Security hardening terintegrasi | `cargo test --test security` (8 passed); `cargo test --test process_runner command_output_is_redacted`; `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test -- --test-threads=1`; `npm run e2e` (13 passed); mutation check klien/PEM/batas body/CORS/output proses | Threat checklist manual ada di `docs/security.md` |
| 2026-10-07 | M5-004 | Audit aksesibilitas dan responsif web terintegrasi | `npm run check` (0 error); `npm run build`; `npm run e2e` (41 passed); suite `accessibility` 3× tanpa flaky (28 passed); mutation check fokus dan reduced-motion | Audit pertama gagal 11/16 sebelum perbaikan |
| 2026-10-07 | M5-005 | Retry/fallback provider yang aman terintegrasi | `cargo test --test model_fallback -- --test-threads=1` (12 passed); `worker_loop` (16) dan `single_worker_flow` (10) lulus; `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test -- --test-threads=1`; `npm run e2e` (41 passed); mutation check 6 kasus router + worker + pemasangan orkestrator | Backoff 500 ms/1 s/2 s…, total tidur ≤ 8 s per model; auth/invalid/context/budget tidak diulang |
| 2026-10-07 | M5-006 | Backup dan restore metadata terintegrasi | `cargo test --test backup -- --test-threads=1` (4 passed); `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test -- --test-threads=1` (49 binary lulus); mutation check 5 kasus skrip + manifest `db.sql` | Restore hanya ke tujuan kosong; secret terdeteksi = arsip tidak dibuat |
| 2026-10-07 | M5-008 | UI operasi (Operations dan Settings) terintegrasi | `npm run check` (0 error, 0 warning); `npm run build`; `node web/src/lib/components/metrics/helpers.test.mjs`; `cargo test --test operations_api` (3 passed); `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test -- --test-threads=1`; `npm run e2e` (50 passed); mutation check backend (3) dan UI (3) | Estimasi berlabel; komponen degraded dijelaskan; secret dan path server tidak tampil |
