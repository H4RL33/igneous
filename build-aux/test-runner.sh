#!/bin/sh
# Cargo starts test binaries through this script (see .cargo/config.toml).
#
# Outside the sealed headless session it hides the desktop's display, session
# bus and runtime directory from tests. A GTK test run by mistake then fails
# to start instead of reaching the desktop. Run GTK tests with
# build-aux/run-ui-tests.sh, which sets IGNEOUS_HEADLESS.
if [ -z "${IGNEOUS_HEADLESS:-}" ]; then
  case "$1" in
    */deps/*)
      unset DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS
      XDG_RUNTIME_DIR="${TMPDIR:-/tmp}/igneous-no-display"
      mkdir -p -m 700 "$XDG_RUNTIME_DIR"
      export XDG_RUNTIME_DIR GDK_BACKEND=wayland
      ;;
  esac
fi
exec "$@"
