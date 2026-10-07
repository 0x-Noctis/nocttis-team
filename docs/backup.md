# Backup dan restore metadata

Yang dicadangkan: database PostgreSQL (task, run, event, usage, plan, audit) dan artifact di disk (diff, bukti verifikasi,
laporan konflik). Skrip: `scripts/backup.sh` dan `scripts/restore.sh`.

**Tidak dicadangkan** (lihat "Batas"): repository Git proyek beserta branch `noctis-integration-<run>` (hasil kerja agent),
worktree sementara, konfigurasi, dan environment/secret.

## Prasyarat
- `bash`, `tar`, `gzip`, `sha256sum`.
- Akses ke PostgreSQL lewat salah satu cara:
  - **Container** (disarankan, host tidak perlu klien Postgres): `NOCTIS_PG_CONTAINER=<nama container>`,
    `NOCTIS_PG_USER`, `NOCTIS_PG_DATABASE` (bawaan `ai_team`/`ai_team`, sesuai `compose.yaml`).
  - **Lokal**: `DATABASE_URL=postgres://…` dengan `pg_dump` dan `psql` di `PATH`.

## Backup
```bash
export NOCTIS_PG_CONTAINER="$(docker compose ps -q postgres)"
scripts/backup.sh --out ./backups --artifacts ./data/artifacts
# -> ./backups/noctis-backup-<UTC>-<pid>.tar.gz  (+ .sha256), izin 0600
```
Isi arsip: `db.sql` (plain SQL, tanpa owner/privilege), `artifacts/` (artifact + metadata), `info.json` (jumlah baris,
versi migration), `MANIFEST.sha256` (SHA-256 setiap file di arsip). Aplikasi boleh tetap berjalan: dump memakai satu snapshot
konsisten, dan artifact disalin SESUDAH dump sehingga setiap baris yang ada di dump punya file-nya.

Backup **gagal** (dan tidak membuat arsip) bila:
| Kode | Penyebab |
|---|---|
| 2 | konfigurasi salah (tidak ada koneksi/direktori artifact), atau database tak terbaca |
| 3 | secret terdeteksi di data: nilai environment ber-nama `*KEY/TOKEN/SECRET/PASSWORD*` (≥ 8 karakter), password di `DATABASE_URL`, atau bentuk umum (kunci privat PEM, `sk-…`, `ghp_…`, `AKIA…`, `xox…`). Pesan hanya menyebut NAMA file, tidak nilainya |
| 4 | artifact di disk tidak cocok dengan checksum di metadata-nya (korup/diubah) |

## Restore (ke environment KOSONG)
```bash
# 1. Hentikan aplikasi. Siapkan database baru yang kosong dan direktori artifact yang kosong/belum ada.
docker compose exec postgres createdb -U ai_team ai_team_restored
# 2. Restore
export NOCTIS_PG_CONTAINER="$(docker compose ps -q postgres)" NOCTIS_PG_DATABASE=ai_team_restored
scripts/restore.sh ./backups/noctis-backup-….tar.gz --artifacts ./data/artifacts-restored
# 3. Arahkan aplikasi ke database/direktori baru (DATABASE_URL, NOCTIS__ARTIFACTS__ROOT), jalankan, lalu:
curl -s localhost:7410/api/v1/health/ready      # harus 200, migrations "ok"
```
Restore memeriksa berurutan: checksum arsip (`.sha256`), bentuk anggota tar (hanya file biasa; tanpa path absolut, `..`, atau
symlink), `MANIFEST.sha256`, checksum tiap artifact, tujuan kosong (database tanpa tabel; direktori artifact kosong), lalu
memuat `db.sql` dalam SATU transaksi dan mencocokkan jumlah baris serta versi migration dengan `info.json`.

| Kode | Penyebab |
|---|---|
| 5 | arsip rusak/diubah/tidak aman (checksum, manifest, atau anggota berbahaya) — belum ada yang ditulis |
| 6 | tujuan tidak kosong — tidak ada yang ditimpa |
| 7 | hasil restore tidak cocok dengan info backup |

Restore sengaja **tidak pernah menimpa** data. Mengganti environment = membuat database dan direktori baru, lalu memindahkan
aplikasi ke sana setelah `health/ready` hijau.

## Drill (bukti prosedur berjalan)
`cargo test --test backup -- --test-threads=1` menjalankan seluruh siklus pada fixture run dengan skema dari migration asli:
backup → restore ke database baru → jumlah baris sama → aplikasi menilai migration valid → artifact terbaca lewat
`ArtifactStore`; ditambah skenario secret, artifact korup, arsip diubah, symlink/path traversal, dan tujuan tidak kosong.
Test membutuhkan container Postgres yang menerbitkan port 55432 **dengan superuser bernama `postgres`** (dilewati bila container tidak ada; penyiapannya di `docs/development.md`). Lakukan drill nyata terhadap salinan
data Anda secara berkala; backup yang tidak pernah di-restore belum terbukti.

## Batas yang diketahui (jujur)
- **Repository Git tidak ikut.** Branch `noctis-integration-<run>` berisi hasil kerja agent yang belum di-merge manusia dan
  hanya ada di repository proyek; cadangkan repository itu terpisah (mis. `git bundle` atau mirror).
- **Arsip tidak dienkripsi.** Isinya data kerja (diff kode, bukti verifikasi, jejak audit). Simpan di lokasi terlindungi dan
  enkripsi sebelum keluar mesin (mis. `age`/`gpg`). Izin file 0600 hanya melindungi di host yang sama.
- Pemindaian secret adalah jaring pengaman (nilai env + bentuk umum), bukan jaminan; redaksi di runtime (`docs/security.md`)
  tetap garis pertahanan utama.
- Tidak ada backup inkremental atau terjadwal; jalankan lewat cron/systemd timer dan atur rotasi sendiri.
- Backup artifact saat aplikasi menulis dapat menyertakan artifact yang barisnya baru ada setelah dump (yatim sementara);
  retensi (`docs/penjelasan` M5-002) membersihkannya setelah masa tenggang.
- `restore.sh` memerlukan hak membuat tabel di database tujuan; dump dibuat tanpa owner agar bisa dimuat oleh role lain.
