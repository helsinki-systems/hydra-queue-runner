{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.05-small";
    nixpkgs-unstable.url = "github:nixos/nixpkgs/nixos-unstable-small";
  };

  outputs =
    {
      self,
      nixpkgs,
      nixpkgs-unstable,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        unstable = import nixpkgs-unstable { inherit system; };
        rustPackages = pkgs.rustPackages_1_88;
        inherit (pkgs) lib;

        nativeBuildInputs = with pkgs; [
          rustPackages.rustc
          llvmPackages.clang
          llvmPackages.lld

          pkg-config

          rustPackages.cargo
          cargo-deny
          cargo-watch
          cargo-outdated
          cargo-machete
          cargo-expand
          systemfd
          rustPackages.clippy
          rustPackages.rustfmt
          sqlx-cli
        ];
        buildInputs = with pkgs; [
          unstable.openssl
          zlib
          protobuf

          unstable.nixVersions.nix_2_31
          nlohmann_json
          libsodium
          boost
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          inherit nativeBuildInputs buildInputs;

          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib/libclang.so";
          RUST_SRC_PATH = "${rustPackages.rustPlatform.rustLibSrc}";
          NIX_CFLAGS_COMPILE = "-Wno-error";
        };
        packages = {
          queue-runner = pkgs.callPackage ./. { };
          default = self.packages.${system}.queue-runner;
        };
        nixosModules = {
          queue-runner = ./runner-module.nix;
          queue-builder = ./builder-module.nix;
        };
        checks =
          let
            testArgs = {
              inherit pkgs self;
            };
          in
          lib.optionalAttrs pkgs.stdenv.isLinux {
            t00-simple = import ./tests/t00-simple.nix testArgs;
            t01-mtls = import ./tests/t01-mtls.nix testArgs;
            t02-mtls-nginx = import ./tests/t02-mtls-nginx.nix testArgs;
          };

      }
    );
}
