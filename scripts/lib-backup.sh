# Fungsi bersama backup.sh dan restore.sh (di-source, bukan dijalankan langsung).
# shellcheck shell=bash

# Koneksi PostgreSQL:
#   - mode container : NOCTIS_PG_CONTAINER=<nama container> (+ NOCTIS_PG_USER, NOCTIS_PG_DATABASE); perintah dijalankan
#                      lewat `docker exec`, jadi host tidak perlu memasang klien PostgreSQL.
#   - mode lokal     : DATABASE_URL=postgres://... dengan pg_dump/psql di PATH.
# Nilai DATABASE_URL tidak pernah dicetak.

set -euo pipefail
umask 077

die() { echo "ERROR: $*" >&2; exit "${2:-1}"; }
info() { echo "==> $*" >&2; }

pg_tool() { # pg_tool <perintah> [argumen...]  (stdin diteruskan)
  local tool="$1"; shift
  if [ -n "${NOCTIS_PG_CONTAINER:-}" ]; then
    docker exec -i "$NOCTIS_PG_CONTAINER" "$tool" -U "${NOCTIS_PG_USER:-ai_team}" -d "${NOCTIS_PG_DATABASE:-ai_team}" "$@"
  else
    [ -n "${DATABASE_URL:-}" ] || die "set NOCTIS_PG_CONTAINER atau DATABASE_URL" 2
    "$tool" "$DATABASE_URL" "$@"
  fi
}

pg_query() { pg_tool psql -X -At -F '|' -v ON_ERROR_STOP=1 -c "$1"; }

# sha256 file -> hex
sha_of() { sha256sum -- "$1" | cut -d' ' -f1; }

# Ambil nilai "checksum" dari <id>.metadata.json (JSON ringkas satu baris dari ArtifactStore).
metadata_checksum() { sed -n 's/.*"checksum":"\([0-9a-f]\{64\}\)".*/\1/p' "$1"; }

# Setiap artifact harus cocok dengan checksum di metadata-nya. Menulis jumlah artifact utuh ke stdout.
verify_artifacts() { # verify_artifacts <direktori>
  local dir="$1" count=0 file id expected actual
  for file in "$dir"/*.artifact; do
    [ -e "$file" ] || continue
    [ -L "$file" ] && die "artifact berupa symlink: $(basename "$file")" 4
    id="$(basename "$file" .artifact)"
    [[ "$id" =~ ^[A-Za-z0-9._-]+$ ]] || die "nama artifact tidak valid: $id" 4
    [ -f "$dir/$id.metadata.json" ] || { info "lewati $id: metadata tidak ada"; continue; }
    expected="$(metadata_checksum "$dir/$id.metadata.json")"
    [ -n "$expected" ] || die "metadata tanpa checksum: $id" 4
    actual="$(sha_of "$file")"
    [ "$expected" = "$actual" ] || die "checksum artifact tidak cocok: $id" 4
    count=$((count + 1))
  done
  echo "$count"
}
