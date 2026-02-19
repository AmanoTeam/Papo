{
  description = "Papo - GTK WhatsApp client";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      rust-overlay,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        # Rust (nightly required for wacore-binary portable_simd)
        rustToolchain = pkgs.rust-bin.nightly."2026-01-30".default.override {
          extensions = [ "rust-analyzer" ];
        };
      in
      {
        formatter = pkgs.nixfmt;

        devShells.default = import ./shell.nix { inherit pkgs rustToolchain; };
        packages.default = pkgs.callPackage ./. { inherit pkgs rustToolchain; };
      }
    );
}
