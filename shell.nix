{
  pkgs,
  rustToolchain,
  ...
}:
let
  manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
in
pkgs.stdenv.mkDerivation {
  name = "${manifest.name}-dev";

  # Compile time dependencies
  nativeBuildInputs = with pkgs; [
    # Rust
    rustToolchain

    gtk4
    meson
    ninja
    parted
    sqlite
    gettext
    libwebp
    openssl
    appstream
    pkg-config
    gdk-pixbuf
    grass-sass
    libadwaita
    libglycin
    gnome-desktop
    glycin-loaders
    libglycin-gtk4
    wrapGAppsHook4
    desktop-file-utils
    gobject-introspection
    rustPlatform.bindgenHook
  ];

  RUST_BACKTRACE = "full";
  RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
}
