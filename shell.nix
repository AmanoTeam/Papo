{
  pkgs,
  rustToolchain,
  ...
}:
let
  manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
in
pkgs.stdenv.mkDerivation {
  name = "${manifest.name}";

  # Compile time dependencies
  nativeBuildInputs = with pkgs; [
    # Rust
    rustToolchain
    rustPlatform.bindgenHook

    # GTK
    gtk4
    libglycin
    gdk-pixbuf
    libadwaita
    libglycin-gtk4
    glycin-loaders
    wrapGAppsHook4
    gobject-introspection

    # Build
    meson
    ninja
    parted
    sqlite
    gettext
    libwebp
    openssl
    appstream
    bubblewrap
    pkg-config
    grass-sass
    gnome-desktop
    desktop-file-utils
  ];

  # Rust variables
  RUST_BACKTRACE = "full";
  RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";

  # Compiler LD variables
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
    pkgs.gcc
    pkgs.libiconv
    pkgs.llvmPackages.llvm
  ];
}
