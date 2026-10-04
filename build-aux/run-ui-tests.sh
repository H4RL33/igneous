#!/bin/sh
# Runs the workspace tests, including GTK ones, in a sealed-off headless
# session (see headless-session.sh). Extra arguments go to `cargo test`.
set -eu
exec "$(dirname "$0")/headless-session.sh" cargo test --workspace "$@"
