{
  pkgs ?
    let
      lock = (builtins.fromJSON (builtins.readFile ./flake.lock)).nodes.nixpkgs.locked;
      nixpkgs = fetchTarball {
        url = "https://github.com/nixos/nixpkgs/archive/${lock.rev}.tar.gz";
        sha256 = lock.narHash;
      };
    in
    import nixpkgs { overlays = [ ]; },
  rustToolchain,
  ...
}:
let
  getLibFolder = pkg: "${pkg}/lib";

  manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
in
pkgs.stdenv.mkDerivation {
  pname = manifest.name;
  version = manifest.version;

  src = pkgs.lib.cleanSource ./.;
  cargoDeps = pkgs.rustPlatform.importCargoLock {
    lockFile = ./Cargo.lock;
  };

  meta = with pkgs.lib; {
    description = "GTK WhatsApp client";
    license = licenses.asl20;
    platforms = platforms.linux;
  };

  # Compile time dependencies
  nativeBuildInputs = with pkgs; [
    # Rust
    rustToolchain
    rustPlatform.cargoSetupHook

    # GTK
    wrapGAppsHook4

    # Build
    cmake
    ninja
    meson
    gettext
    appstream
    grass-sass
    pkg-config
    desktop-file-utils
  ];
  dontUseCmakeConfigure = true;

  # Runtime dependencies which will be shipped
  # with nix package
  buildInputs = with pkgs; [
    # Rust
    rustPlatform.bindgenHook

    # GTK
    gtk4
    glib
    libglycin
    gdk-pixbuf
    libadwaita
    libglycin-gtk4
    glycin-loaders
    adwaita-icon-theme

    # Build
    sqlite
    libwebp
    openssl
    gnome-desktop
    desktop-file-utils
  ];

  # Compiler LD variables
  NIX_LDFLAGS = "-L${(getLibFolder pkgs.libiconv)}";
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
    pkgs.gcc
    pkgs.libiconv
    pkgs.llvmPackages.llvm
  ];
}
