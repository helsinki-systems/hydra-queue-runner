{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.05-small";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        inherit (pkgs) rustPackages lib;

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
          rustPackages.clippy
          rustPackages.rustfmt
          sqlx-cli
          tokio-console
        ];
        buildInputs = with pkgs; [
          openssl
          zlib
          protobuf

          nixVersions.nix_2_29
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
