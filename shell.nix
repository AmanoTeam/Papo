{ pkgs, ... }:
let
  # Manifest via Cargo.toml
  manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
in
pkgs.stdenv.mkDerivation {
  name = "${manifest.name}-dev";

  # Compile time dependencies
  nativeBuildInputs = with pkgs; [
    # Hail the Nix
    nil
    nixfmt
    statix
    deadnix

    # Rust
    rustc
    cargo
    clippy
    rustfmt
    cargo-watch
    rust-analyzer

    # Other compile time dependencies
    openssl

    # Gnome related
    gtk4
    meson
    ninja
    parted
    gettext
    appstream
    pkg-config
    gdk-pixbuf
    libadwaita
    gnome-desktop
    wrapGAppsHook4
    desktop-file-utils
    gobject-introspection
    rustPlatform.bindgenHook
  ];

  # Set Environment Variables
  RUST_BACKTRACE = "full";
  RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
}
