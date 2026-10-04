#!/bin/sh
# Runs a command in a sealed-off headless GNOME session, for tests and
# screenshots. Nothing in it can reach the desktop you're using:
#
# - its own XDG runtime, config, data, cache and state directories, so no
#   shared sockets (keyring, gvfs, PipeWire, accessibility) or settings files;
# - a D-Bus session bus that starts no services (test-session.conf);
# - in-memory GSettings, local-only GIO, no portals, no accessibility bus;
# - no sound server, so GTK's error bell is silent;
# - GTK's simple input method instead of Wayland text-input. In GTK 4.22 an
#   editor focused before the session's text-input object exists is never
#   cleared from the input method's state, and GTK crashes on a later
#   text-input event after that editor is destroyed. Headless mutter sets
#   text-input up late, so tests would hit it.
#
# Usage: build-aux/headless-session.sh COMMAND [ARGS...]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
sandbox=$(mktemp -d "${TMPDIR:-/tmp}/igneous-session.XXXXXX")
cleanup() { rm -rf "$sandbox"; }
trap cleanup EXIT INT TERM
for d in runtime config data cache state; do mkdir -p "$sandbox/$d"; done
chmod 700 "$sandbox/runtime"

unset DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS
export XDG_RUNTIME_DIR="$sandbox/runtime" XDG_CONFIG_HOME="$sandbox/config" \
  XDG_DATA_HOME="$sandbox/data" XDG_CACHE_HOME="$sandbox/cache" XDG_STATE_HOME="$sandbox/state" \
  GSETTINGS_BACKEND=memory GIO_USE_VFS=local GTK_A11Y=none NO_AT_BRIDGE=1 GDK_DEBUG=no-portals \
  GTK_IM_MODULE=gtk-im-context-simple

dbus-run-session --config-file="$here/test-session.conf" -- sh -c '
  mutter --headless --wayland --no-x11 --virtual-monitor 1280x1024 \
    --wayland-display wayland-test >"$XDG_RUNTIME_DIR/mutter.log" 2>&1 &
  mutter_pid=$!
  trap "kill $mutter_pid 2>/dev/null; wait $mutter_pid 2>/dev/null" EXIT
  i=0
  while [ ! -S "$XDG_RUNTIME_DIR/wayland-test" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
  if [ ! -S "$XDG_RUNTIME_DIR/wayland-test" ]; then
    echo "headless mutter didn'"'"'t start:" >&2
    cat "$XDG_RUNTIME_DIR/mutter.log" >&2
    exit 1
  fi
  WAYLAND_DISPLAY=wayland-test GDK_BACKEND=wayland "$@"
' sh "$@"
