#!/bin/sh
# Validates the metainfo file, failing on errors but not on warnings (such as
# the homepage URL, which doesn't exist until the project is published).
set -u
output=$("$1" validate --no-net --explain "$2" 2>&1)
status=$?
printf '%s\n' "$output"
if printf '%s\n' "$output" | grep -q '^E:'; then
  exit 1
fi
[ "$status" -eq 0 ] || [ "$status" -eq 3 ]
