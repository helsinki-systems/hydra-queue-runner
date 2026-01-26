{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.11-small";
    nixpkgs-unstable.url = "github:nixos/nixpkgs/nixos-unstable-small";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      nixpkgs-unstable,
      flake-utils,
      treefmt-nix,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        unstable = import nixpkgs-unstable { inherit system; };
        treefmtEval = treefmt-nix.lib.evalModule pkgs ./treefmt.nix;

        inherit (unstable) rustPackages;
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
          zlib
          protobuf

          unstable.nixVersions.nix_2_32
          nlohmann_json
          libsodium
          boost
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [
            treefmtEval.config.build.wrapper
          ]
          ++ nativeBuildInputs;
          inherit buildInputs;

          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib/libclang.so";
          RUST_SRC_PATH = "${rustPackages.rustPlatform.rustLibSrc}";
          NIX_CFLAGS_COMPILE = "-Wno-error";
        };
        packages = {
          queue-runner = pkgs.callPackage ./. { };
          default = self.packages.${system}.queue-runner;
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
            formatting = treefmtEval.config.build.check self;
          };
        formatter = treefmtEval.config.build.wrapper;
      }
    )
    // {
      darwinModules = {
        queue-builder = ./darwin-builder-module.nix;
      };
      nixosModules = {
        queue-runner = ./runner-module.nix;
        queue-builder = ./linux-builder-module.nix;
      };
    };
}
