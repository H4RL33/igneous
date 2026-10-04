# Igneous

A GNOME app for reading and editing Markdown vaults, including Obsidian vaults.

Igneous is approaching its first release: Live Preview editing, links and backlinks, properties, search, Bases, the graph, Git sync and the linter all work. [`docs/user-guide.md`](docs/user-guide.md) explains how to use it. [`PROJECT.md`](PROJECT.md) describes what it's for and the plan, and [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md) how it's built.

## Building

You need Rust (stable), GTK 4.22, libadwaita 1.9, GtkSourceView 5.18+, Meson and `blueprint-compiler` 0.22+. Without a system `blueprint-compiler`, fetch the pinned copy:

```sh
meson subprojects download blueprint-compiler
```

For development:

```sh
cargo run                        # the Devel build, dev.h4rl3y.igneous.Devel
cargo run -- path/to/vault       # open a vault (or a note inside one)
```

To build and install:

```sh
meson setup _build
meson compile -C _build
meson install -C _build          # add --skip-subprojects when Blueprint came from the subproject
```

## Testing

```sh
build-aux/run-ui-tests.sh        # every test; GTK tests run in a sealed headless session
build-aux/ci.sh                  # everything CI checks
```

Run GTK tests only through these scripts. `build-aux/headless-session.sh` keeps test sessions away from your desktop's sockets, services and settings. As a safety net, Cargo starts every test binary through `build-aux/test-runner.sh`, which hides your display and session bus from tests run any other way, so a stray `cargo test` can't open windows on your desktop.

## Licence

BSD-3-Clause. See [`LICENSE`](LICENSE).
