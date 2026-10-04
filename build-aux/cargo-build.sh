#!/bin/sh
# Usage: cargo-build.sh CARGO BUILT_BINARY OUTPUT [CARGO_OPTIONS...]
# Builds with Cargo and copies the binary to where Meson expects it.
set -eu
cargo="$1"
built="$2"
output="$3"
shift 3
"$cargo" build "$@"
cp "$built" "$output"
