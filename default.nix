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
    outputHashes = {
      "wacore-0.2.0" = "sha256-BA+xdc1+iAK8yDAOH+k0xpu6SHc2b8QNn64dsjpGRj0=";
      "libglycin-gtk4-rebind-0.0.1" = "sha256-QYMFay6HHQxdAV3xZy29SkSAt2zU/yMLvDp6f3EwfvY=";
    };
  };

  meta = with pkgs.lib; {
    description = "GTK WhatsApp client";
    license = licenses.asl20;
    platforms = platforms.linux;
  };

  # Compile time dependencies
  nativeBuildInputs = with pkgs; [
    ninja
    meson
    gettext
    appstream
    grass-sass
    pkg-config
    rustToolchain
    wrapGAppsHook4
    desktop-file-utils
    rustPlatform.cargoSetupHook
  ];

  # Runtime dependencies which will be shipped
  # with nix package
  buildInputs = with pkgs; [
    gtk4
    glib
    sqlite
    libwebp
    openssl
    libadwaita
    gdk-pixbuf
    libglycin
    gnome-desktop
    glycin-loaders
    libglycin-gtk4
    adwaita-icon-theme
    desktop-file-utils
    rustPlatform.bindgenHook
  ];

  # Compiler LD variables
  NIX_LDFLAGS = "-L${(getLibFolder pkgs.libiconv)}";
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
    pkgs.gcc
    pkgs.libiconv
    pkgs.llvmPackages.llvm
  ];
}
