conf = configuration_data()
conf.set('APP_ID', application_id)
conf.set('SCHEMA_PATH', '/@0@/'.format(application_id.replace('.', '/')))
conf.set('VERSION', meson.project_version())
conf.set('URL', project_url)
conf.set('SCREENSHOTS', screenshots_url)

desktop_file = i18n.merge_file(
  input: configure_file(
    input: '@0@.desktop.in'.format(base_id),
    output: '@0@.desktop.in'.format(application_id),
    configuration: conf,
  ),
  output: '@0@.desktop'.format(application_id),
  type: 'desktop',
  po_dir: '../po',
  install: true,
  install_dir: datadir / 'applications',
)
if desktop_file_validate.found()
  test('validate-desktop', desktop_file_validate, args: [desktop_file])
endif

metainfo_file = i18n.merge_file(
  input: configure_file(
    input: '@0@.metainfo.xml.in'.format(base_id),
    output: '@0@.metainfo.xml.in'.format(application_id),
    configuration: conf,
  ),
  output: '@0@.metainfo.xml'.format(application_id),
  po_dir: '../po',
  install: true,
  install_dir: datadir / 'metainfo',
)
if appstreamcli.found()
  test(
    'validate-metainfo',
    find_program(meson.project_source_root() / 'build-aux' / 'validate-metainfo.sh'),
    args: [appstreamcli.full_path(), metainfo_file],
  )
endif

configure_file(
  input: '@0@.gschema.xml.in'.format(base_id),
  output: '@0@.gschema.xml'.format(application_id),
  configuration: conf,
  install: true,
  install_dir: datadir / 'glib-2.0' / 'schemas',
)

install_data(
  'icons/hicolor/scalable/apps/@0@.svg'.format(base_id),
  install_dir: datadir / 'icons' / 'hicolor' / 'scalable' / 'apps',
  rename: '@0@.svg'.format(application_id),
)
install_data(
  'icons/hicolor/symbolic/apps/@0@-symbolic.svg'.format(base_id),
  install_dir: datadir / 'icons' / 'hicolor' / 'symbolic' / 'apps',
  rename: '@0@-symbolic.svg'.format(application_id),
)
