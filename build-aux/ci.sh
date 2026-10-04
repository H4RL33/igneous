#!/bin/sh
# Everything CI checks, runnable locally. GTK tests run in a private headless
# session (see run-ui-tests.sh).
set -eu
cd "$(dirname "$0")/.."

echo "== format"
cargo fmt --all -- --check

echo "== lint"
cargo clippy --workspace --all-targets -- -D warnings

echo "== tests"
build-aux/run-ui-tests.sh

if cargo deny --version >/dev/null 2>&1; then
  echo "== licences and advisories"
  cargo deny check
else
  echo "== cargo-deny not installed; skipping licence check"
fi

echo "== meson build"
builddir="${MESON_BUILDDIR:-_build}"
if [ -d "$builddir" ]; then
  meson setup --reconfigure "$builddir" >/dev/null
else
  meson setup "$builddir" >/dev/null
fi
meson compile -C "$builddir"
meson test -C "$builddir" --suite igneous
