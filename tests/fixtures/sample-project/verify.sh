#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

first=$($root/reset.sh >/dev/null && git -C "$root/.fixture-repo" rev-parse HEAD)
second=$($root/reset.sh >/dev/null && git -C "$root/.fixture-repo" rev-parse HEAD)
test "$first" = "$second"
"$root/test.sh"
node "$root/verify-scenarios.mjs"

repo="$root/.fixture-repo"
sed -i 's/price.toFixed(2)/price.toFixed(1)/' "$repo/src/frontend.js"
if (cd "$repo" && node --test --test-name-pattern=frontend >/dev/null 2>&1); then
  printf '%s\n' 'expected frontend verification to fail' >&2
  exit 1
fi
git -C "$repo" restore src/frontend.js

git -C "$repo" switch --quiet -c price-25
sed -i "s/price: 24/price: 25/" "$repo/src/products.js"
git -C "$repo" add src/products.js
GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.invalid \
GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.invalid \
GIT_AUTHOR_DATE=2000-01-02T00:00:00Z GIT_COMMITTER_DATE=2000-01-02T00:00:00Z \
  git -C "$repo" commit --quiet -m 'change lamp price to 25'
git -C "$repo" switch --quiet main
git -C "$repo" switch --quiet -c price-26
sed -i "s/price: 24/price: 26/" "$repo/src/products.js"
git -C "$repo" add src/products.js
GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.invalid \
GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.invalid \
GIT_AUTHOR_DATE=2000-01-03T00:00:00Z GIT_COMMITTER_DATE=2000-01-03T00:00:00Z \
  git -C "$repo" commit --quiet -m 'change lamp price to 26'
if git -C "$repo" merge --quiet price-25 >/dev/null 2>&1; then
  printf '%s\n' 'expected price branches to conflict' >&2
  exit 1
fi
git -C "$repo" merge --abort

"$root/reset.sh" >/dev/null
"$root/test.sh"
printf 'deterministic commit: %s\n' "$second"
