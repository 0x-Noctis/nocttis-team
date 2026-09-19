#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo="$root/.fixture-repo"

test -d "$repo/.git" || "$root/reset.sh" >/dev/null
cd "$repo"
node --test
