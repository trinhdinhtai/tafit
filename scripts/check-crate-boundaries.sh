#!/usr/bin/env bash
# Fails if a crate depends on something its layer forbids (ADR-0004, ADR-0005).
# Checks normal and build dependencies for every target platform; dev
# dependencies are free to cross layers.
set -euo pipefail

status=0

# deps <package> [cargo tree flags...]: one dependency name per line.
deps() {
  local package=$1
  shift
  cargo tree --package "$package" --edges normal,build --target all \
    --prefix none --format '{p}' "$@" | awk '{print $1}' | sort -u
}

# forbid <description> <regex> <package> [cargo tree flags...]
forbid() {
  local description=$1 pattern=$2
  shift 2
  local hits
  hits=$(deps "$@" | grep -E "$pattern" || true)
  if [[ -n "$hits" ]]; then
    echo "error: $description, but it depends on:" >&2
    echo "$hits" | sed 's/^/  /' >&2
    status=1
  else
    echo "ok: $description"
  fi
}

gpui='^gpui'
forbid "tafit-core must not depend on GPUI" "$gpui" tafit-core
forbid "tafit-state must not depend on GPUI" "$gpui" tafit-state
forbid "tafit-ui-kit must not depend on tafit-core or tafit-state" \
  '^tafit-(core|state)$' tafit-ui-kit
forbid "the headless build (tafit-app without default features) must not pull in GPUI" \
  "$gpui" tafit-app --no-default-features

exit $status
