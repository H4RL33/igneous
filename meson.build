project(
  'igneous',
  'rust',
  version: '0.1.0',
  meson_version: '>= 1.0.0',
  license: 'BSD-3-Clause',
)

gnome = import('gnome')
i18n = import('i18n')

base_id = 'io.github.h4rl33.igneous'

# The metainfo's homepage, issue tracker and screenshot links are built from
# it (screenshots are data/screenshots/*.png, served raw from the main branch).
project_url = 'https://github.com/H4RL33/igneous'
screenshots_url = project_url / 'raw' / 'main' / 'data' / 'screenshots'

dependency('gtk4', version: '>= 4.22')
dependency('libadwaita-1', version: '>= 1.9')
dependency('gtksourceview-5', version: '>= 5.18')

cargo = find_program('cargo')
blueprint_compiler = find_program('blueprint-compiler', version: '>= 0.22')
desktop_file_validate = find_program('desktop-file-validate', required: false)
appstreamcli = find_program('appstreamcli', required: false)

if get_option('profile') == 'development'
  profile = 'Devel'
  application_id = '@0@.Devel'.format(base_id)
  git_rev = run_command('git', 'rev-parse', '--short', 'HEAD', check: false).stdout().strip()
  version = '@0@-@1@'.format(meson.project_version(), git_rev != '' ? git_rev : 'devel')
else
  profile = ''
  application_id = base_id
  version = meson.project_version()
endif

prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
datadir = prefix / get_option('datadir')
localedir = prefix / get_option('localedir')

subdir('data')
subdir('po')
subdir('src')

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
