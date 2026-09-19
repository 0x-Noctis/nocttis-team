# Rancangan Sistem Tim AI Agent untuk Coding

## 1. Ringkasan

Sistem ini adalah WebApp lokal atau self-hosted untuk mengelola tim AI agent seperti tim engineering. Lead Agent memecah permintaan menjadi task, menentukan dependency dan role, lalu scheduler menjalankan worker secara paralel jika ruang kerja dan file yang disentuh tidak bertabrakan. Hasil kerja selalu berupa artifact, patch Git, review, dan bukti test—bukan percakapan panjang antarapara agent.

Fokus MVP:

- penggunaan token terkendali;
- beberapa agent dapat bekerja paralel;
- setiap agent mendapat konteks minimum;
- dukungan endpoint OpenAI-compatible;
- perubahan kode terisolasi dengan Git worktree;
- review dan verification wajib sebelum integrasi;
- seluruh status, biaya, dan keputusan terlihat dari WebApp;
- proses dapat pulih setelah server mati.

## 2. Sasaran dan Batas

### Sasaran MVP

1. Pengguna mendaftarkan repository lokal dan membuat project run.
2. Lead Agent membuat rencana berbentuk task DAG.
3. Maksimal empat worker berjalan bersamaan dengan batas yang dapat diubah.
4. Worker hanya menerima task contract, skill terpilih, dan source relevan.
5. Worker bekerja dalam Git worktree terpisah.
6. Reviewer menerima diff dan bukti, bukan transcript worker.
7. Verifier menjalankan command deterministik seperti test, lint, dan build.
8. Integrator menggabungkan task yang lolos seluruh quality gate.
9. WebApp menampilkan progres, dependency, penggunaan token, biaya, artifact, diff, dan log event.
10. Provider dan model dapat diganti lewat konfigurasi OpenAI-compatible.

### Bukan Sasaran MVP

- deployment produksi otomatis;
- akses repository melalui SaaS multi-tenant;
- voice/chat antarpuluhan agent;
- vector database;
- Kubernetes atau message broker;
- agent yang mengubah kebijakan atau skill sendiri;
- merge otomatis saat terjadi konflik semantik;
- dukungan semua variasi API setiap vendor.

Tambahkan setelah metrik membuktikan kebutuhan.

## 3. Stack

| Bagian | Pilihan | Alasan |
|---|---|---|
| Backend | Rust stable, Axum, Tokio | HTTP dan concurrency kuat dengan satu service |
| Database | PostgreSQL, SQLx | koordinasi worker, locking, recovery, dan pertumbuhan multi-instance |
| Frontend | SvelteKit, TypeScript, adapter-static | UI reaktif; output statis dapat dilayani Axum |
| Styling | CSS native | belum perlu component framework |
| Realtime | Server-Sent Events (SSE) | dashboard hanya perlu aliran event satu arah |
| Model API | HTTP OpenAI-compatible | bebas vendor tanpa SDK khusus |
| HTTP client | reqwest | streaming dan koneksi HTTP matang |
| Serialization | serde, serde_json | format data internal dan eksternal |
| Version control | Git CLI dan Git worktree | native, dapat diaudit, tidak perlu library Git |
| Artifact | filesystem | patch, log, context snapshot, dan hasil test mudah diperiksa |
| Observability | tracing, tracing-subscriber | structured log standar Rust |
| Secrets | environment variable | API key tidak masuk DB atau artifact |

Sistem awal berupa satu service backend, satu bundle frontend, dan PostgreSQL. Tidak ada microservice.

## 4. Arsitektur

```text
Browser (SvelteKit static app)
              |
          HTTP + SSE
              |
+-------------v--------------------------------------------------+
|                        Rust / Axum                              |
|                                                                |
|  API ---- Project Service ---- Task Service ---- Event Store    |
|                   |                 |                            |
|                   |             DAG Scheduler                   |
|                   |                 |                            |
|                   |         Agent Run Controller                |
|                   |          /      |       \                    |
|              Context Builder |  Model Gateway  Tool Runner      |
|                   |          |      |            |               |
|              Skill Registry  | OpenAI-compatible |              |
|                              |                   |               |
|                   Artifact Store ---- Git Worktree Manager       |
+---------------------------|-------------------|------------------+
                            |                   |
                       PostgreSQL         Repository + Git
```

### Control Plane

Menangani project, task DAG, budget, policy, state transition, scheduling, approval, dan recovery. Lead Agent hanya bekerja dengan project brief, ringkasan repository, status task, serta artifact keputusan.

### Execution Plane

Menjalankan agent, tool, command, dan Git worktree. Worker tidak dapat mengubah task graph, budget, policy, atau workspace agent lain.

### Prinsip Utama

- tools mengerjakan operasi deterministik;
- model membuat keputusan yang membutuhkan penalaran;
- database menjadi sumber kebenaran status;
- Git menjadi sumber kebenaran perubahan kode;
- artifact menjadi media handoff;
- event log menjadi jejak audit;
- transcript bukan memori proyek.

## 5. Struktur Role

Role dibuat sesuai kebutuhan task, bukan selalu aktif.

| Role | Tanggung jawab | Hak utama |
|---|---|---|
| Lead | membuat dan memperbarui task DAG, menetapkan role dan budget | metadata project dan task |
| Architect | membuat kontrak atau keputusan lintas komponen | baca peta repository, tulis artifact desain |
| Worker | menghasilkan patch untuk scope tertentu | baca/edit worktree miliknya, jalankan tool terbatas |
| Reviewer | menilai diff terhadap contract dan acceptance criteria | baca diff/source, tanpa edit |
| Verifier | menjalankan pemeriksaan deterministik | test/lint/build, tanpa edit |
| Integrator | menerapkan patch yang lolos dan menangani konflik mekanis | worktree integrasi dan Git |

Lead tidak menulis implementasi pada alur normal. Untuk task kecil, sistem boleh memakai alur ringkas `Worker → Reviewer → Verifier` tanpa Architect.

## 6. Alur Project

```text
User membuat project
        |
Repository discovery (tool, bukan agent jika cukup)
        |
Lead membuat proposed plan
        |
Human menyetujui atau mengubah plan
        |
Scheduler mengambil task READY
        |
Worktree + context dibuat
        |
Worker menghasilkan patch dan self-check
        |
Reviewer menerima task contract + diff + hasil check
        |
Verifier menjalankan command yang ditentukan
        |
Integrator menerapkan patch ke branch integrasi
        |
Regression check
        |
Project selesai atau meminta keputusan manusia
```

### Paralelisme

Task boleh berjalan paralel jika:

- semua dependency selesai;
- scope file tidak overlap;
- kontrak bersama sudah disetujui;
- slot worker dan budget tersedia;
- tidak ada lease aktif pada scope yang sama.

Dua agent dengan role sama tetap mendapat task berbeda. Untuk file atau komponen sama, gunakan satu Worker dan satu Reviewer, bukan dua editor.

## 7. Task Contract

Setiap task wajib memiliki kontrak berikut sebelum masuk status `READY`:

```json
{
  "id": "BE-014",
  "project_id": "P-001",
  "title": "Buat endpoint produk",
  "role": "backend_engineer",
  "objective": "Implementasikan pembuatan produk sesuai kontrak API",
  "depends_on": ["ARCH-003"],
  "allowed_paths": ["src/products/**", "tests/products/**"],
  "context_refs": ["artifact://decisions/api-contract-v1"],
  "acceptance_criteria": [
    "Payload tidak valid menghasilkan HTTP 400",
    "SKU duplikat menghasilkan HTTP 409",
    "Test produk lulus"
  ],
  "verification_commands": ["cargo test products"],
  "limits": {
    "max_input_tokens": 30000,
    "max_output_tokens": 8000,
    "max_tool_calls": 40,
    "max_attempts": 2,
    "timeout_seconds": 1200
  }
}
```

Task tanpa acceptance criteria, allowed paths, atau verification tidak boleh dijadwalkan.

## 8. State Machine

```text
DRAFT
  |
PLANNED
  |
READY <-------------------------------+
  |                                   |
ASSIGNED                               |
  |                                   |
RUNNING                                |
  |                                   |
SELF_CHECK                             |
  |                                   |
REVIEW --------> CHANGES_REQUESTED ----+
  |
VERIFY ----------> FAILED ------------+
  |                                   |
INTEGRATE --------> CONFLICT --> NEEDS_HUMAN
  |
DONE
```

Status terminal lain: `CANCELLED` dan `FAILED_FINAL`.

Aturan:

- Worker hanya dapat mengajukan `SELF_CHECK` dan `REVIEW`.
- Reviewer menentukan `CHANGES_REQUESTED` atau `VERIFY`.
- Verifier menentukan `FAILED` atau `INTEGRATE`.
- Integrator menentukan `DONE`, `CONFLICT`, atau rollback.
- Control plane memvalidasi setiap transisi dalam transaksi DB.
- Restart server mengembalikan run terlantar ke antrean recovery.

## 9. Model Gateway OpenAI-Compatible

### Dukungan MVP

- `POST /v1/chat/completions`;
- respons biasa dan streaming SSE;
- `messages`;
- `tools` dan `tool_calls`;
- `temperature` jika provider mendukung;
- `max_tokens` atau alias yang ditentukan konfigurasi;
- pembacaan `usage` jika tersedia;
- custom `base_url`, header, dan model;
- timeout, retry terbatas, dan fallback.

`/v1/responses` ditambahkan setelah ada provider target yang memerlukannya.

### Konfigurasi

```toml
[[providers]]
id = "primary"
base_url = "https://api.example.com/v1"
api_key_env = "PRIMARY_API_KEY"
request_timeout_seconds = 180

[[providers.models]]
id = "coding-large"
remote_name = "vendor/model-name"
class = "coding"
context_window = 131072
max_output_tokens = 16384
supports_tools = true
supports_parallel_tools = false
supports_streaming = true

[routing]
lead = "reasoning"
architect = "reasoning"
worker = "coding"
reviewer = "reasoning"
summarizer = "cheap"
```

API key hanya dibaca dari environment variable. Endpoint konfigurasi WebApp menampilkan nama variable dan status tersedia, tidak pernah nilai secret.

### Request Internal

```rust
struct ModelRequest {
    project_id: String,
    task_id: String,
    agent_run_id: String,
    model_class: String,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    limits: ModelLimits,
}
```

Gateway melakukan:

1. memilih model sesuai class dan capability;
2. memeriksa token serta budget project/task;
3. mengirim request;
4. menormalisasi content, tool call, finish reason, error, dan usage;
5. mencatat latency serta token;
6. retry hanya untuk timeout, rate limit, atau provider unavailable;
7. fallback hanya sebelum side effect yang belum tercatat atau setelah checkpoint aman.

### Compatibility Probe

Provider baru diuji dengan request kecil untuk:

- autentikasi;
- respons biasa;
- streaming;
- tool call;
- usage;
- timeout;
- format error.

Capability hasil probe disimpan. Klaim provider tidak dianggap cukup.

## 10. Context dan Skill

### Context Builder

Context Builder menyusun prompt dari:

1. system policy ringkas;
2. role instruction;
3. task contract;
4. keputusan proyek yang dirujuk;
5. skill yang cocok;
6. potongan source relevan;
7. hasil tool terbaru yang masih dibutuhkan.

Source ditemukan dengan `rg`, daftar simbol, import, test terkait, dan riwayat file. MVP belum memakai embedding.

### Batas Awal

| Role | Maksimum input default |
|---|---:|
| Lead | 12.000 token |
| Architect | 20.000 token |
| Worker | 30.000 token |
| Reviewer | 20.000 token |
| Verifier | 8.000 token |

Jika limit terlampaui, sistem mengecilkan source excerpt atau meminta task dipecah. Sistem tidak langsung menaikkan context window.

### Skill Registry

Skill berupa Markdown dengan metadata kecil:

```yaml
---
id: rust-axum-review
roles: [backend_engineer, reviewer]
triggers: [rust, axum]
summary: Review handler, extractor, error mapping, and shared state usage.
estimated_tokens: 2400
version: 1
---
```

Lead hanya melihat metadata. Isi lengkap dimuat jika role, teknologi, trigger, dan budget cocok. Maksimal satu atau dua skill khusus per run pada MVP.

### Handoff

Agent berikutnya menerima artifact ringkas:

```json
{
  "task_id": "BE-014",
  "status": "ready_for_review",
  "base_commit": "abc123",
  "files_changed": ["src/products/create.rs"],
  "decisions": ["Duplicate SKU maps to HTTP 409"],
  "checks": [{"command": "cargo test products", "status": "passed"}],
  "known_risks": [],
  "patch_ref": "artifact://patches/BE-014.diff"
}
```

Tool output besar disimpan sebagai artifact. Prompt hanya menerima referensi dan ringkasannya.

## 11. Tool Runner dan Keamanan

Model tidak mendapat shell mentah. Model meminta tool terstruktur, lalu Tool Runner memvalidasi role, task, path, argumen, timeout, dan approval.

### Tool MVP

- `list_files`;
- `search_code`;
- `read_file` dengan line range dan batas byte;
- `apply_patch`;
- `git_diff`;
- `git_status`;
- `run_check` dari allowlist task;
- `submit_artifact`;
- `request_human`.

Tidak ada tool `run_shell(command: string)` untuk model.

### Batas Keamanan

- path dinormalisasi dan wajib berada dalam worktree task;
- symlink yang keluar workspace ditolak;
- edit dibatasi oleh `allowed_paths`;
- command verification berasal dari config project atau persetujuan manusia;
- environment child process memakai allowlist;
- secret tidak diteruskan ke agent atau command;
- output dibatasi ukuran dan waktu;
- operasi network dari tool worker nonaktif secara default;
- penghapusan data, deployment, credential, dan perubahan di luar workspace memerlukan approval manusia;
- semua tool call dan hasilnya dicatat sebagai event.

Sandbox OS yang lebih kuat dapat ditambahkan kemudian. MVP tidak boleh mengklaim isolasi keamanan penuh hanya karena memakai worktree.

## 12. Git dan Isolasi Kerja

Layout runtime:

```text
data/
├── artifacts/<project-id>/<task-id>/
└── worktrees/<project-id>/<task-id>/
```

Per task:

1. catat base commit;
2. buat branch dan worktree sementara;
3. terapkan allowed path lease;
4. Worker membuat perubahan;
5. simpan `git diff --binary` sebagai artifact;
6. Reviewer dan Verifier bekerja pada snapshot sama;
7. Integrator menerapkan patch ke branch integrasi;
8. hapus worktree setelah retention period.

MVP tidak melakukan `git push`. User tetap mengontrol remote repository.

## 13. Data Model

Tabel minimum:

```text
projects
project_runs
tasks
task_dependencies
agent_runs
reviews
artifacts
events
model_usage
tool_calls
file_leases
approvals
providers
models
skills
```

Kolom penting:

### `tasks`

```text
id, project_run_id, parent_id, role, title, objective, status,
allowed_paths_json, acceptance_criteria_json, verification_json,
input_token_limit, output_token_limit, max_attempts, priority,
created_at, updated_at
```

### `agent_runs`

```text
id, task_id, role, provider_id, model_id, attempt, status,
worktree_path, base_commit, started_at, finished_at, error_code
```

### `model_usage`

```text
id, agent_run_id, request_id, input_tokens, cached_tokens,
output_tokens, estimated, cost_micros, latency_ms, created_at
```

### `events`

```text
id, project_run_id, task_id, actor_type, actor_id, event_type,
payload_json, created_at
```

Gunakan migration SQL yang disimpan di repository dan dijalankan saat deployment. Event log append-only; state utama tetap berada di tabel domain agar query sederhana. Gunakan `jsonb` hanya untuk payload fleksibel; kolom yang sering difilter atau diurutkan tetap bertipe eksplisit dan diberi index.

## 14. HTTP API Backend

Prefix: `/api/v1`.

### Project

```text
POST   /projects
GET    /projects
GET    /projects/:id
POST   /projects/:id/discover
POST   /projects/:id/runs
```

### Run dan Plan

```text
GET    /runs/:id
POST   /runs/:id/plan
POST   /runs/:id/approve-plan
POST   /runs/:id/pause
POST   /runs/:id/resume
POST   /runs/:id/cancel
GET    /runs/:id/events
GET    /runs/:id/events/stream
```

### Task

```text
GET    /runs/:id/tasks
GET    /tasks/:id
GET    /tasks/:id/artifacts
GET    /tasks/:id/diff
POST   /tasks/:id/retry
POST   /tasks/:id/cancel
POST   /tasks/:id/approve
```

### Provider

```text
GET    /providers
POST   /providers
PATCH  /providers/:id
POST   /providers/:id/probe
GET    /models
```

### Skills

```text
GET    /skills
POST   /skills/reload
```

Mutation menerima idempotency key. Error memakai bentuk konsisten:

```json
{
  "error": {
    "code": "INVALID_TASK_TRANSITION",
    "message": "Task REVIEW tidak dapat kembali ke RUNNING tanpa changes request",
    "details": {}
  }
}
```

## 15. WebApp Svelte

### Halaman

1. **Projects** — daftar repository, status, dan penggunaan terakhir.
2. **New Project** — path repository, branch dasar, tujuan, budget, concurrency.
3. **Project Run** — kanban task, DAG, progres, token, biaya, dan controls.
4. **Task Detail** — contract, context refs, attempt, tool calls, diff, test, review, artifact.
5. **Approvals** — keputusan berisiko yang menunggu manusia.
6. **Providers** — endpoint, model, capability, probe result, status secret.
7. **Skills** — metadata skill, ukuran, trigger, dan status aktif.
8. **Settings** — data directory, retention, concurrency, default budget.

### Tampilan Run

```text
+---------------------------------------------------------------+
| Project: Inventory API   Running   124k/300k token   3 workers |
+----------------------+----------------------+-----------------+
| READY                | RUNNING              | REVIEW / VERIFY |
| FE-12 Product form   | BE-14 Create product | BE-13 Auth      |
| QA-09 Contract test  | FE-11 Product list   | FE-10 Layout    |
+----------------------+----------------------+-----------------+
| Events | Costs | DAG | Approvals | Artifacts                  |
+---------------------------------------------------------------+
```

SSE mengirim event baru. Setelah reconnect, client mengirim last event ID dan mengambil event yang tertinggal.

### State Frontend

Gunakan Svelte stores lokal dan `fetch`. Belum perlu state library tambahan. Server tetap sumber kebenaran; optimistic update hanya untuk aksi UI yang aman.

### Aksesibilitas

- seluruh aksi dapat dipakai dengan keyboard;
- status tidak dibedakan hanya lewat warna;
- diff dan event memiliki label tekstual;
- focus berpindah ke dialog approval;
- animasi mengikuti `prefers-reduced-motion`.

## 16. Scheduler

Scheduler berjalan dalam loop Tokio:

1. cari task `READY` berdasarkan priority;
2. periksa dependency, budget, slot, dan file lease;
3. klaim task dalam transaksi PostgreSQL;
4. buat worktree dan context;
5. jalankan Agent Run Controller;
6. simpan heartbeat;
7. lepaskan slot dan lease saat selesai;
8. emit event untuk setiap perubahan.

Klaim task menggunakan `SELECT ... FOR UPDATE SKIP LOCKED` dalam transaksi agar beberapa scheduler tidak mengambil task sama. File lease memakai constraint unik dan waktu kedaluwarsa. PostgreSQL advisory lock hanya dipakai untuk operasi global singkat seperti recovery project, bukan sebagai pengganti state task.

Contoh klaim task:

```sql
WITH candidate AS (
    SELECT id
    FROM tasks
    WHERE status = 'READY'
    ORDER BY priority DESC, created_at
    FOR UPDATE SKIP LOCKED
    LIMIT 1
)
UPDATE tasks
SET status = 'ASSIGNED', updated_at = now()
WHERE id = (SELECT id FROM candidate)
RETURNING *;
```

Recovery saat startup:

- run tanpa heartbeat melewati timeout ditandai `INTERRUPTED`;
- cek worktree, patch, dan tool side effect;
- lanjutkan dari checkpoint aman atau buat attempt baru;
- jangan mengulang tool side effect tanpa idempotency check.

## 17. Budget dan Observability

Budget berlaku pada project run, task, attempt, dan request.

Aturan awal:

- warning pada 70%;
- checkpoint dan context compression pada 85%;
- berhenti pada 100%;
- maksimal dua attempt default;
- reserve project 15% untuk recovery dan integrasi;
- request ditolak sebelum dikirim jika estimasi input melewati limit;
- angka provider yang tidak memberi usage ditandai `estimated`.

Dashboard menampilkan:

- input, cached, dan output token;
- biaya per provider/model/role/task;
- latency;
- retry;
- tool call;
- context bytes dan estimated tokens;
- success rate;
- konflik file;
- waktu antre dan waktu eksekusi.

Log tidak menyimpan API key, authorization header, atau seluruh prompt secara default. Context snapshot dapat disimpan hanya melalui setting debug dan diberi retention pendek.

## 18. Struktur Repository

```text
ai-team/
├── Cargo.toml
├── crates/
│   ├── server/          # Axum routes dan startup
│   ├── core/            # domain, state machine, policy
│   ├── store/           # PostgreSQL dan artifact store
│   ├── model-gateway/   # OpenAI-compatible client
│   ├── scheduler/       # DAG scheduler dan recovery
│   └── runner/          # worktree, tools, process runner
├── web/                 # SvelteKit static app
├── migrations/
├── skills/
├── config/
│   └── example.toml
├── tests/
│   └── fixtures/
└── docs/
```

Enam crate adalah batas atas awal, bukan kewajiban. Mulai dengan satu crate jika pemisahan belum memberi nilai; ekstrak saat dependency boundary nyata muncul.

## 19. Konfigurasi Runtime

```toml
[server]
bind = "127.0.0.1:7410"
data_dir = "./data"

[scheduler]
max_parallel_agents = 4
heartbeat_seconds = 10
stale_after_seconds = 60

[budgets]
default_project_tokens = 300000
default_task_input_tokens = 30000
default_task_output_tokens = 8000
reserve_percent = 15

[git]
worktree_root = "./data/worktrees"
retention_hours = 24

[artifacts]
root = "./data/artifacts"
max_tool_output_bytes = 1048576
```

Environment minimum:

```text
DATABASE_URL=postgres://ai_team:...@127.0.0.1:5432/ai_team
PRIMARY_API_KEY=...
RUST_LOG=info
```

Connection pool memiliki batas tetap dan timeout. Backend tidak menerima traffic sebelum migration tervalidasi dan koneksi PostgreSQL sehat.

## 20. Quality Gates

### Task Gate

- allowed paths tidak dilanggar;
- patch dapat diterapkan;
- acceptance criteria diperiksa;
- task-specific check lulus;
- review diterima;
- tidak ada secret baru pada diff;
- penggunaan budget tercatat.

### Integration Gate

- seluruh dependency task selesai;
- patch diterapkan bersih;
- test integrasi lulus;
- build project lulus;
- working tree integrasi konsisten;
- approval manusia selesai jika dibutuhkan.

Deteksi secret MVP memakai pola terkonfigurasi dan pemeriksaan nama file. Jangan klaim hasilnya menggantikan secret scanner khusus.

## 21. Strategi Pengujian

### Unit

- validasi state transition;
- DAG cycle detection;
- budget accounting;
- path scope matching;
- provider response normalization;
- tool policy;
- context truncation.

### Integration

- PostgreSQL task claim atomik dengan `SKIP LOCKED`;
- file lease dan scheduler multi-instance;
- SSE reconnect;
- mock OpenAI-compatible streaming dan tool call;
- Git worktree lifecycle;
- patch review, verification, dan integration;
- restart recovery.

### End-to-End Fixture

Repository kecil berisi backend dan frontend dummy. Skenario wajib:

1. dua task independen berjalan paralel;
2. dua task overlap ditahan scheduler;
3. review meminta perubahan lalu attempt kedua lulus;
4. provider timeout memicu retry aman;
5. server mati dan run pulih;
6. budget habis menghentikan task;
7. test gagal mencegah integrasi.

### Tolok Ukur

Bandingkan single-agent dan sistem ini pada lima tugas tetap:

- success rate;
- token per task selesai;
- waktu selesai;
- retry;
- konflik;
- test pass rate;
- intervensi manusia;
- biaya.

## 22. Tahapan Pembangunan

### Fase 0 — Baseline

- pilih satu repository fixture;
- rekam hasil lima tugas dengan satu agent;
- tetapkan target token, kualitas, dan waktu.

Selesai jika baseline dapat diulang.

### Fase 1 — Model dan Data Foundation

- setup Rust workspace, Svelte app, PostgreSQL, dan migration;
- provider config dan compatibility probe;
- normalized model response;
- project, task, artifact, event, dan usage store;
- halaman Providers sederhana.

Selesai jika request streaming dan tool call dari provider target teruji.

### Fase 2 — Single Worker Pipeline

- task state machine;
- Context Builder berbasis `rg`;
- Tool Runner allowlist;
- Git worktree;
- Worker, Reviewer, Verifier berurutan;
- task detail, diff, event, dan usage pada WebApp.

Selesai jika satu task dapat menghasilkan patch terverifikasi end-to-end.

### Fase 3 — Lead dan DAG

- Lead menghasilkan proposed plan terstruktur;
- schema validation dan cycle detection;
- plan approval manusia;
- dependency scheduling;
- project run board dan DAG view.

Selesai jika project multi-task berjalan sesuai dependency.

### Fase 4 — Parallel Workers

- worker slots;
- file lease;
- worktree paralel;
- integration branch;
- conflict handling;
- pause, resume, cancel, dan recovery.

Selesai jika dua sampai empat task independen berjalan tanpa overlap.

### Fase 5 — Hardening

- approval gate;
- secret redaction;
- retention;
- provider fallback;
- performance metrics;
- E2E suite penuh;
- dokumentasi operasi dan backup.

Selesai jika server restart tidak kehilangan status dan seluruh skenario wajib lulus.

## 23. Kriteria Keberhasilan MVP

- minimal 50% penurunan median input token dibanding baseline untuk task yang sama;
- minimal 80% task fixture selesai tanpa intervensi manual;
- tidak ada edit di luar allowed paths;
- tidak ada dua worker memegang lease scope sama;
- seluruh task `DONE` memiliki review, verification result, patch, dan usage;
- restart server tidak kehilangan project atau status task;
- provider dapat diganti lewat konfigurasi tanpa perubahan kode agent;
- empat worker paralel dapat dipantau dan dihentikan dari WebApp;
- setiap integrasi gagal ditolak tanpa merusak branch dasar.

## 24. Keputusan yang Perlu Dikunci Sebelum Coding

1. Sistem hanya berjalan lokal atau juga di server internal.
2. OS target pertama; rekomendasi Linux.
3. Provider OpenAI-compatible pertama untuk compatibility fixture.
4. Repository fixture untuk E2E.
5. Apakah user harus menyetujui setiap plan atau hanya plan berisiko.
6. Batas biaya dan token default.
7. Command project yang boleh dijalankan worker.
8. Apakah branch integrasi boleh membuat commit lokal otomatis.

Rekomendasi awal: Linux lokal, satu pengguna, approval pada setiap plan, tanpa `git push`, commit integrasi lokal opsional, dan network tool worker nonaktif.

## 25. Langkah Pertama

Urutan kerja pertama:

1. kunci delapan keputusan di atas;
2. buat repository fixture kecil;
3. catat baseline single-agent;
4. implementasikan compatibility probe;
5. implementasikan state machine dan storage;
6. selesaikan single-worker pipeline sebelum paralelisme dan Lead Agent.

Jangan mulai dari dashboard penuh. WebApp awal cukup untuk provider probe, membuat task manual, melihat event, diff, test, dan token. Lead Agent dan paralelisme ditambahkan setelah pipeline tunggal terbukti benar.
