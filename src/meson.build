cargo_options = [
  '--manifest-path', meson.project_source_root() / 'Cargo.toml',
  '--target-dir', meson.project_build_root() / 'target',
  '--package', 'igneous',
]
if get_option('profile') == 'default'
  cargo_options += ['--release']
  rust_target = 'release'
else
  rust_target = 'debug'
endif

cargo_env = environment()
cargo_env.set('BLUEPRINT_COMPILER', blueprint_compiler.full_path())
# Blueprint's wrapper needs its module on the path when it's a subproject.
cargo_env.prepend('PYTHONPATH', meson.project_source_root() / 'subprojects' / 'blueprint-compiler')
cargo_env.set('IGNEOUS_APP_ID', application_id)
cargo_env.set('IGNEOUS_PROFILE', profile)
cargo_env.set('IGNEOUS_VERSION', version)
cargo_env.set('IGNEOUS_LOCALEDIR', localedir)

custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: bindir,
  env: cargo_env,
  command: [
    meson.project_source_root() / 'build-aux' / 'cargo-build.sh',
    cargo,
    meson.project_build_root() / 'target' / rust_target / meson.project_name(),
    '@OUTPUT@',
    cargo_options,
  ],
)
