project(
  'papo',
  'rust',
  version: '0.1.0',
  meson_version: '>= 0.59',
  license: 'Apache-2.0',
)

i18n = import('i18n')
gnome = import('gnome')

base_id = 'com.amanoteam.Papo'
application_id = base_id

dependency('glib-2.0', version: '>= 2.82')
dependency('gio-2.0', version: '>= 2.82')
dependency('gtk4', version: '>= 4.20.2')
dependency('libadwaita-1', version: '>= 1.8.0')

# Please keep these dependencies sorted.

find_program('glib-compile-resources')
glib_compile_schemas = find_program('glib-compile-schemas')
desktop_file_validate = find_program('desktop-file-validate', required: false)
appstreamcli = find_program('appstreamcli', required: false)
cargo = find_program('cargo')

version = meson.project_version()

prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
localedir = prefix / get_option('localedir')

datadir = prefix / get_option('datadir')
pkgdatadir = datadir / meson.project_name()
iconsdir = datadir / 'icons'
podir = meson.project_source_root() / 'po'
gettext_package = meson.project_name()

if get_option('profile') == 'development'
  profile = 'Devel'
  application_id += '.Devel'
elif get_option('profile') == 'beta'
  profile = 'Beta'
  version_suffix = '-beta'
else
  profile = 'Stable'
  version_suffix = ''
endif

if profile == 'Devel'
  vcs_tag = run_command('git', 'rev-parse', '--short', 'HEAD', check: false).stdout().strip()
  if vcs_tag == ''
    version_suffix = '-devel'
  else
    version_suffix = '-@0@'.format(vcs_tag)
  endif

  release_date = run_command('git', 'show', '-s', '--format=%cI', check: true).stdout().strip()

  # Setup pre-commit hook for ensuring coding style is always consistent
  message('Setting up git pre-commit hook..')
  run_command('cp', '-f', 'hooks/pre-commit.hook', '.git/hooks/pre-commit', check: false)
endif

meson.add_dist_script(
  'build-aux/dist-vendor.sh',
  meson.project_build_root() / 'meson-dist' / meson.project_name()
  + '-'
  + version,
  meson.project_source_root(),
)

subdir('data')
subdir('po')
subdir('src')

gnome.post_install(
  gtk_update_icon_cache: true,
  glib_compile_schemas: true,
  update_desktop_database: true,
)
