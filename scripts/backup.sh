#!/usr/bin/env bash
# Backup metadata Noctis: dump PostgreSQL + artifact + manifest checksum dalam satu arsip tar.gz.
#
# Pemakaian:
#   scripts/backup.sh [--out DIR] [--artifacts DIR]
# Lingkungan: lihat scripts/lib-backup.sh (NOCTIS_PG_CONTAINER atau DATABASE_URL).
# Kode keluar: 0 sukses, 2 konfigurasi, 3 secret terdeteksi di arsip, 4 artifact korup.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib-backup.sh
source "$HERE/lib-backup.sh"

OUT="${NOCTIS_BACKUP_DIR:-./backups}"
ARTIFACTS="${NOCTIS_ARTIFACT_ROOT:-./data/artifacts}"
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --artifacts) ARTIFACTS="$2"; shift 2 ;;
    *) die "argumen tidak dikenal: $1" 2 ;;
  esac
done
[ -d "$ARTIFACTS" ] || die "direktori artifact tidak ada: $ARTIFACTS" 2
mkdir -p "$OUT"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir "$STAGE/artifacts"

info "dump database"
# --no-owner/--no-privileges: dump portabel ke environment dengan role berbeda. pg_dump memakai satu snapshot konsisten.
pg_tool pg_dump --no-owner --no-privileges --format=plain > "$STAGE/db.sql"

info "salin dan verifikasi artifact"
count="$(verify_artifacts "$ARTIFACTS")"
for file in "$ARTIFACTS"/*.artifact; do
  [ -e "$file" ] || continue
  id="$(basename "$file" .artifact)"
  [ -f "$ARTIFACTS/$id.metadata.json" ] || continue
  cp -- "$file" "$STAGE/artifacts/$id.artifact"
  cp -- "$ARTIFACTS/$id.metadata.json" "$STAGE/artifacts/$id.metadata.json"
done

info "catat info"
IFS='|' read -r tasks attempts artifact_rows events version < <(pg_query \
  "SELECT (SELECT count(*) FROM tasks),(SELECT count(*) FROM agent_runs),(SELECT count(*) FROM artifacts),(SELECT count(*) FROM events),(SELECT COALESCE(max(version),0) FROM _sqlx_migrations)")
[ -n "$version" ] || die "gagal membaca database (jumlah/versi migration kosong)" 2
cat > "$STAGE/info.json" <<JSON
{"format":1,"created_at":"$(date -u +%Y-%m-%dT%H:%M:%SZ)","migration_version":$version,"tasks":$tasks,"attempts":$attempts,"artifact_rows":$artifact_rows,"events":$events,"artifact_files":$count}
JSON

info "pindai secret"
# Pola nilai konkret dari environment (nama berisi KEY/TOKEN/SECRET/PASSWORD, panjang >= 8) dan bentuk rahasia umum.
# Pola ditulis ke file 0600 (bukan argv) supaya nilai tidak muncul di daftar proses; hasil pemindaian hanya memuat nama file.
LITERALS="$(mktemp)"; SHAPES="$(mktemp)"
trap 'rm -rf "$STAGE" "$LITERALS" "$SHAPES"' EXIT
while IFS='=' read -r name value; do
  if [[ "${name^^}" =~ (KEY|TOKEN|SECRET|PASSWORD|PASSWD) ]] && [ "${#value}" -ge 8 ]; then
    printf '%s\n' "$value" >> "$LITERALS"
  fi
done < <(env)
if [ -n "${DATABASE_URL:-}" ]; then
  pw="${DATABASE_URL#*://*:}"; pw="${pw%%@*}"
  if [ "${#pw}" -ge 8 ] && [ "$pw" != "$DATABASE_URL" ]; then printf '%s\n' "$pw" >> "$LITERALS"; fi
fi
cat > "$SHAPES" <<'PATTERNS'
-----BEGIN [A-Z ]*PRIVATE KEY-----
sk-[A-Za-z0-9_-]{20,}
gh[pousr]_[A-Za-z0-9]{30,}
AKIA[0-9A-Z]{16}
xox[bpas]-[0-9A-Za-z-]{20,}
PATTERNS
hits=""
if [ -s "$LITERALS" ]; then hits="$(grep -rlFf "$LITERALS" "$STAGE" || true)"; fi
hits="$hits$(grep -rlEf "$SHAPES" "$STAGE" || true)"
if [ -n "$hits" ]; then
  echo "$hits" | sed "s|$STAGE/||" | sort -u | sed 's/^/  berisi secret: /' >&2
  die "secret terdeteksi; arsip TIDAK dibuat. Bersihkan sumber datanya lalu ulangi." 3
fi

info "manifest checksum"
( cd "$STAGE" && find . -type f ! -name MANIFEST.sha256 | LC_ALL=C sort | xargs -d '\n' sha256sum > MANIFEST.sha256 )

ARCHIVE="$OUT/noctis-backup-$(date -u +%Y%m%dT%H%M%SZ)-$$.tar.gz"
tar --owner=0 --group=0 --numeric-owner --sort=name -C "$STAGE" -czf "$ARCHIVE" .
( cd "$OUT" && sha256sum "$(basename "$ARCHIVE")" > "$(basename "$ARCHIVE").sha256" )
info "selesai: $ARCHIVE (tasks=$tasks attempts=$attempts artifacts=$count)"
echo "$ARCHIVE"
