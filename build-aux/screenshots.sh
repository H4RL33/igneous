#!/bin/sh
# Regenerates data/screenshots/*.png (the metainfo's screenshots) from the
# synthetic vault in tests/screenshots.rs, in the sealed headless session.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
shots=$(mktemp -d "${TMPDIR:-/tmp}/igneous-shots.XXXXXX")
trap 'rm -rf "$shots"' EXIT INT TERM
IGNEOUS_SCREENSHOTS="$shots" "$here/headless-session.sh" \
  cargo test --manifest-path "$root/Cargo.toml" --test screenshots -- --ignored
mkdir -p "$root/data/screenshots"
for png in "$shots"/*.png; do
  out="$root/data/screenshots/$(basename "$png")"
  if command -v oxipng >/dev/null; then
    oxipng -q -o 4 --strip safe --out "$out" "$png"
  elif python3 -c 'import PIL' 2>/dev/null; then
    python3 -c 'import sys; from PIL import Image; Image.open(sys.argv[1]).save(sys.argv[2], optimize=True)' "$png" "$out"
  else
    cp "$png" "$out"
  fi
done
ls -l "$root/data/screenshots"
