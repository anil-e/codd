cargo = find_program('cargo', required: true)
sh = find_program('sh', required: true)

cargo_target_dir = meson.current_build_dir() / 'cargo-target'

env = environment()
env.set('CARGO_TARGET_DIR', cargo_target_dir)
env.set('CODD_APP_ID', app_id)
env.set('CODD_APP_SETTINGS_PATH', '/io/github/anil_e/codd/')
env.set('CODD_CONNECTION_STATE_SCHEMA_ID', 'io.github.anil_e.Codd.connection-state')
env.set('CODD_CONNECTION_STATE_PATH_PREFIX', '/io/github/anil_e/codd/connection-state/')
env.set('CODD_GETTEXT_PACKAGE', meson.project_name())
env.set('CODD_LOCALEDIR', get_option('prefix') / get_option('localedir'))
env.set('CODD_PKGDATADIR', pkgdatadir)

custom_target(
  'codd',
  build_by_default: true,
  output: 'codd',
  command: [
    sh,
    '-c',
    'cargo build --release --manifest-path "$1" && install -Dm755 "$2/release/codd" "$3"',
    'sh',
    meson.project_source_root() / 'Cargo.toml',
    cargo_target_dir,
    '@OUTPUT@',
  ],
  env: env,
  capture: false,
  install: true,
  install_dir: get_option('bindir'),
)
