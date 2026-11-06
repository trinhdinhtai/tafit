#!/usr/bin/env bash
# perf-snapshots.sh <dir>: makes sure <dir> holds the real Repository
# snapshots the nightly benchmarks open (git, bun, chromium), each a blobless
# clone of its full history up to a pinned tag, checked out at that tag.
#
# A snapshot already at its pinned commit is left alone, so a cached <dir>
# costs nothing. Changing a pin here changes the CI cache key (this file's
# hash), so the snapshots are rebuilt once.
set -euo pipefail

dir=${1:?usage: perf-snapshots.sh <dir>}
mkdir -p "$dir"

# name url tag
snapshots=(
  "git https://github.com/git/git v2.53.0"
  "bun https://github.com/oven-sh/bun bun-v1.3.0"
  "chromium https://chromium.googlesource.com/chromium/src 140.0.7339.0"
)

for line in "${snapshots[@]}"; do
  read -r name url tag <<<"$line"
  target="$dir/$name"
  if [[ -d "$target/.git" ]] &&
    [[ "$(git -C "$target" rev-parse HEAD)" == "$(git -C "$target" rev-parse "refs/tags/$tag^{commit}" 2>/dev/null)" ]]; then
    echo "ok: $name is at $tag"
    continue
  fi
  echo "cloning $name at $tag (blobless; this can take a while)"
  rm -rf "$target"
  # Blobless keeps every commit and tree for history and graph benchmarks;
  # only the checked-out tree's blobs are downloaded.
  git -c advice.detachedHead=false clone --quiet --filter=blob:none --single-branch --branch "$tag" "$url" "$target"
done
