# Keamanan Noctis Team (MVP)

Dokumen ini menyatakan apa yang dilindungi, bagaimana, di mana diuji, dan **apa yang tidak dilindungi**.
Model ancaman MVP: satu operator tepercaya di mesin sendiri; model/provider LLM dan isi repository dianggap
**tidak tepercaya**; tidak ada autentikasi pengguna di API (lihat "Batas" di bawah).

## Kontrol dan pengujian

| Ancaman | Kontrol | Lokasi | Pengujian |
|---|---|---|---|
| Path traversal oleh tool worker | `validate_relative` menolak `..`, absolut, `-opsi`, karakter kontrol; `ToolPolicy` mencocokkan allowed paths | `src/runner/policy.rs` | `tests/tool_policy.rs`, `tests/security.rs` (allowlist) |
| Symlink keluar worktree | komponen path diperiksa `symlink_metadata`; patch ke symlink/rename/copy ditolak; context builder menolak symlink | `src/runner/tools`, `src/runner/git.rs`, `src/context` | `tests/tool_policy.rs`, `tests/git_worktree.rs`, `tests/context_builder.rs` |
| Command injection | tanpa shell; executable harus nama polos (bukan `sh/bash/…`, tanpa `/`); argumen tanpa `; \| & $ < > \`` dan karakter kontrol; hanya perintah allowlist | `src/runner/process.rs` | `tests/process_runner.rs`, `tests/security.rs` |
| Environment bocor ke perintah | hanya key allowlist yang valid yang diteruskan | `src/runner/process.rs` | `tests/security.rs` |
| Eksfiltrasi lewat jaringan dari perintah | container `--network none`, `--read-only`, `--cap-drop ALL`, `no-new-privileges`, batas cpu/memori/pids | `src/runner/container.rs` | `tests/process_runner.rs` |
| Rahasia di prompt/keluaran | redaksi pola rahasia + nilai API key terdaftar di setiap request ke provider dan pada stdout/stderr perintah | `src/security/redact.rs` | `tests/security.rs`, `tests/process_runner.rs` |
| File rahasia dibaca ke konteks | `.env*`, `*.pem`, `*.key`, `id_rsa`, `.netrc`, dll. ditolak oleh context builder | `src/context/mod.rs` | `tests/context_builder.rs` |
| Payload besar | batas body global 256 KiB (task/project 64 KiB) | `src/security/limits.rs`, `src/api/*` | `tests/security.rs` (batas global; batas 64 KiB khusus task/project belum diuji tersendiri) |
| CORS terlalu longgar | origin eksplisit dari `server.cors_allowed_origins`, tanpa wildcard/path, maksimal 8; invalid = server tidak start | `src/security/cors.rs`, `src/config.rs` | `tests/security.rs` |
| SSRF lewat base URL provider | tujuan diperiksa pada SETIAP panggilan (probe, Lead, worker, reviewer): host publik, atau host persis di `NOCTIS_PROVIDER_HOST_ALLOWLIST`; DNS di-resolve ulang tiap panggilan; gagal tertutup | `src/security/ssrf.rs`, `src/model/openai/tools.rs` | `tests/model_tools.rs`, `tests/provider_api.rs`, unit test `ssrf` |
| API terekspos ke jaringan | server menolak start bila `server.bind` bukan loopback kecuali `NOCTIS_ALLOW_NON_LOOPBACK=1`; peringatan "tanpa autentikasi" dicatat di log setiap start | `src/config.rs`, `src/main.rs` | unit test `config` |
| Log membocorkan data | log JSON hanya memuat method + path + request_id; query/header/body tidak dicatat | `src/observability.rs` | `tests/logging.rs` |
| Agent mengubah branch utama / push | tidak ada kode push; integrasi hanya ke branch `noctis-integration-<run>` | `src/runner/integration_git.rs` | `tests/integrator.rs` |

## Redaksi rahasia: cara kerja dan batasnya
Dikenali: blok kunci privat PEM, nilai yang didaftarkan (API key provider didaftarkan otomatis saat klien dibuat),
token berformat dikenal (`sk-`, `ghp_`, `AKIA…`, `xox…`, `AIza…`, JWT), nilai setelah `Bearer`, dan penugasan
`nama=nilai`/`"nama": "nilai"` untuk nama yang memuat password/secret/token/api_key.

Sengaja **tidak** dikenali: string acak heksa/base64 polos (hash commit adalah data sah) dan nilai < 8 karakter.
Akibatnya redaksi ini **mengurangi** kebocoran, bukan menjaminnya: rahasia dengan format tak dikenal yang ada di
repository tetap bisa sampai ke provider. Jangan menaruh rahasia di repository yang dikerjakan agent.

## Batas yang diketahui (jujur)
- **Tidak ada autentikasi/otorisasi pengguna di API** (ditunda untuk MVP lokal atas keputusan pemilik). Syaratnya: server
  wajib tetap di loopback (ditegakkan: start ditolak bila `server.bind` bukan loopback kecuali `NOCTIS_ALLOW_NON_LOOPBACK=1`
  yang hanya boleh dipakai di balik pembatas lain, mis. port Docker ke 127.0.0.1) dan setiap start mencatat peringatan jelas.
  Jangan diekspos ke jaringan tanpa reverse proxy yang menambah autentikasi dan TLS. `/api/v1/metrics` dan `/health/*` juga terbuka.
- Identitas aksi manusia (`actor_id`) dicatat tetapi tidak diverifikasi.
- Isolasi perintah bergantung pada Docker daemon lokal; akses ke socket Docker setara akses root.
- Model dapat menulis kode berbahaya yang lolos review; verifikasi berjalan di container tanpa jaringan, tetapi
  hasil di cabang integrasi harus ditinjau manusia sebelum di-merge.
- Redaksi tidak berlaku untuk artifact diff/patch (itu data kerja yang harus utuh) maupun isi database.
- Rate limiting per pengguna belum ada.

## Checklist ancaman manual (sebelum rilis)
- [ ] `server.bind` masih localhost, atau ada proxy dengan autentikasi di depannya.
- [ ] `server.cors_allowed_origins` hanya berisi origin UI yang dipakai.
- [ ] API key hanya di environment, bukan di repository/konfigurasi.
- [ ] Image runner (`NOCTIS_RUNNER_IMAGE`) berasal dari sumber tepercaya dan di-pin.
- [ ] Repository target tidak berisi rahasia.
- [ ] `cargo audit` dan `npm audit` dijalankan (belum otomatis di CI MVP).
