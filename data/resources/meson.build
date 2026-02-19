# Stylesheet
# We accept grass (a Rust SASS compiler) or sass (the official dart SASS compiler).
sass_bin = find_program('grass', required: false)
sass_options = []
if not sass_bin.found()
    # Require SASS as we need at least one compiler.
    sass_bin = find_program('sass')

    if sass_bin.found()
        # these options are not supported by grass.
        sass_options += ['--no-error-css', '--no-source-map']
    endif
endif

# Keep ordered alphabetically.
scss_deps = files(
    'stylesheet/_common.scss',
    'stylesheet/_vendor.scss',
)

stylesheet = custom_target(
    'stylesheet',
    input: 'style.scss',
    output: 'style.css',
    command: [sass_bin, sass_options, '@INPUT@', '@OUTPUT@'],
    depend_files: scss_deps,
)

# Resources
resources = gnome.compile_resources(
  'resources',
  'resources.gresource.xml',
  gresource_bundle: true,
  source_dir: meson.current_build_dir(),
  install: true,
  install_dir: pkgdatadir,
  dependencies: stylesheet,
)
