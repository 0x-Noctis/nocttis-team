#!/usr/bin/env bash
# Restore backup Noctis ke environment KOSONG (database tanpa tabel dan direktori artifact kosong/belum ada).
#
# Pemakaian:
#   scripts/restore.sh <arsip.tar.gz> [--artifacts DIR]
# Lingkungan: lihat scripts/lib-backup.sh; NOCTIS_PG_* / DATABASE_URL menunjuk database TUJUAN.
# Sengaja menolak menimpa data yang ada: untuk mengganti environment, buat database baru dan direktori baru.
# Kode keluar: 0 sukses, 2 konfigurasi, 5 arsip tidak utuh/tidak aman, 6 tujuan tidak kosong, 7 hasil restore tidak cocok.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib-backup.sh
source "$HERE/lib-backup.sh"

ARCHIVE="${1:-}"; [ -n "$ARCHIVE" ] || die "pemakaian: restore.sh <arsip.tar.gz> [--artifacts DIR]" 2
shift
ARTIFACTS="${NOCTIS_ARTIFACT_ROOT:-./data/artifacts}"
while [ $# -gt 0 ]; do
  case "$1" in
    --artifacts) ARTIFACTS="$2"; shift 2 ;;
    *) die "argumen tidak dikenal: $1" 2 ;;
  esac
done
[ -f "$ARCHIVE" ] || die "arsip tidak ditemukan" 2

info "verifikasi checksum arsip"
if [ -f "$ARCHIVE.sha256" ]; then
  ( cd "$(dirname "$ARCHIVE")" && sha256sum -c --quiet "$(basename "$ARCHIVE").sha256" ) >&2 || die "checksum arsip tidak cocok" 5
else
  info "peringatan: $(basename "$ARCHIVE").sha256 tidak ada; hanya manifest di dalam arsip yang diperiksa"
fi

info "periksa isi arsip"
# Hanya file dan direktori biasa, tanpa path absolut atau `..` (mencegah arsip berbahaya menulis di luar tujuan).
while IFS= read -r line; do
  type="${line:0:1}"; name="${line##* }"
  case "$type" in -|d) ;; *) die "anggota arsip bukan file biasa: $name" 5 ;; esac
  case "$name" in /*|*..*) die "path anggota arsip tidak aman: $name" 5 ;; esac
done < <(tar -tvzf "$ARCHIVE")

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
tar --no-same-owner --no-same-permissions -xzf "$ARCHIVE" -C "$STAGE"
for required in db.sql info.json MANIFEST.sha256; do [ -f "$STAGE/$required" ] || die "arsip tanpa $required" 5; done

info "verifikasi manifest dan artifact"
( cd "$STAGE" && sha256sum -c --quiet MANIFEST.sha256 ) >&2 || die "manifest tidak cocok: arsip rusak atau diubah" 5
mkdir -p "$STAGE/artifacts"
files="$(verify_artifacts "$STAGE/artifacts")" || exit 5

info "periksa tujuan kosong"
tables="$(pg_query "SELECT count(*) FROM information_schema.tables WHERE table_schema='public'")"
[ "$tables" = "0" ] || die "database tujuan tidak kosong ($tables tabel); buat database baru" 6
if [ -d "$ARTIFACTS" ] && [ -n "$(ls -A "$ARTIFACTS")" ]; then die "direktori artifact tujuan tidak kosong" 6; fi

info "restore database (satu transaksi)"
pg_tool psql -X -q -v ON_ERROR_STOP=1 --single-transaction < "$STAGE/db.sql" > /dev/null

info "restore artifact"
mkdir -p "$ARTIFACTS"
if [ "$files" -gt 0 ]; then cp -- "$STAGE"/artifacts/* "$ARTIFACTS"/; fi
[ "$(verify_artifacts "$ARTIFACTS")" = "$files" ] || die "artifact hasil restore tidak utuh" 7

info "cocokkan dengan info backup"
field() { sed -n "s/.*\"$1\":\([0-9]*\).*/\1/p" "$STAGE/info.json"; }
IFS='|' read -r tasks attempts artifact_rows events version < <(pg_query \
  "SELECT (SELECT count(*) FROM tasks),(SELECT count(*) FROM agent_runs),(SELECT count(*) FROM artifacts),(SELECT count(*) FROM events),(SELECT COALESCE(max(version),0) FROM _sqlx_migrations)")
for pair in "tasks:$tasks" "attempts:$attempts" "artifact_rows:$artifact_rows" "events:$events" "migration_version:$version"; do
  key="${pair%%:*}"; got="${pair##*:}"
  [ "$(field "$key")" = "$got" ] || die "$key tidak cocok (backup $(field "$key"), hasil $got)" 7
done
info "restore selesai: tasks=$tasks attempts=$attempts artifact=$files"
