#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo="$root/.fixture-repo"

rm -rf "$repo"
mkdir "$repo"
cp -R "$root/template/." "$repo/"
git -C "$repo" init --quiet --initial-branch=main
git -C "$repo" add .
GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.invalid \
GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.invalid \
GIT_AUTHOR_DATE=2000-01-01T00:00:00Z GIT_COMMITTER_DATE=2000-01-01T00:00:00Z \
  git -C "$repo" commit --quiet -m 'fixture: initial state'

printf '%s\n' "$repo"
