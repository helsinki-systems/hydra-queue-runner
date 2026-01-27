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
        nix = unstable.nixVersions.nix_2_33;
        inherit (pkgs) lib;

        nativeBuildInputs = with pkgs; [
          pkg-config
          protobuf
          makeWrapper
        ];
        # same for shell and pkg
        buildInputs = with pkgs; [
          zlib
          protobuf

          nix
          nlohmann_json
          libsodium
          boost
        ];

        src = pkgs.nix-gitignore.gitignoreSource [ ] (
          lib.sources.sourceFilesBySuffices (lib.cleanSource ./.) [
            ".rs"
            ".cpp"
            ".h"
            ".toml"
            ".lock"
            ".md"
            ".proto"
            ".json"
          ]
        );
        cargoLock = {
          lockFile = ./Cargo.lock;
          outputHashes = {
            "nix-diff-0.1.0" = "sha256-heUqcAnGmMogyVXskXc4FMORb8ZaK6vUX+mMOpbfSUw=";
          };
        };
        meta = {
          description = "Hydra Queue-Runner implemented in rust";
          homepage = "https://github.com/helsinki-systems/queue-runner";
          license = [ lib.licenses.gpl3 ];
          maintainers = [ lib.maintainers.conni2461 ];
          platforms = lib.platforms.all;
        };
        hydra-queue-runner =
          {
            withOtel ? false,
          }:
          rustPackages.rustPlatform.buildRustPackage {
            name = "hydra-queue-runner";
            inherit src;
            __structuredAttrs = true;
            strictDeps = true;

            inherit
              cargoLock
              nativeBuildInputs
              buildInputs
              ;

            buildAndTestSubdir = "queue-runner";
            buildFeatures = lib.optional withOtel "otel";
            doCheck = false;

            postInstall = ''
              wrapProgram $out/bin/queue-runner \
                --prefix PATH : ${lib.makeBinPath [ nix ]} \
                --set-default JEMALLOC_SYS_WITH_MALLOC_CONF "background_thread:true,narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0,abort_conf:true"
            '';

            meta = meta // {
              mainProgram = "queue-runner";
            };
          };

        hydra-queue-builder =
          {
            withOtel ? false,
          }:
          rustPackages.rustPlatform.buildRustPackage {
            name = "hydra-queue-builder";
            inherit src;
            __structuredAttrs = true;
            strictDeps = true;

            inherit
              cargoLock
              nativeBuildInputs
              buildInputs
              ;

            buildAndTestSubdir = "builder";
            buildFeatures = lib.optional withOtel "otel";
            doCheck = false;

            postInstall = ''
              wrapProgram $out/bin/builder \
                --prefix PATH : ${lib.makeBinPath [ nix ]} \
                --set-default JEMALLOC_SYS_WITH_MALLOC_CONF "background_thread:true,narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0,abort_conf:true"
            '';

            meta = meta // {
              mainProgram = "builder";
            };
          };
      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs =
            with pkgs;
            [
              rustPackages.rustc
              llvmPackages.clang
              llvmPackages.lld

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

              treefmtEval.config.build.wrapper
            ]
            ++ nativeBuildInputs;
          inherit buildInputs;

          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib/libclang.so";
          RUST_SRC_PATH = "${rustPackages.rustPlatform.rustLibSrc}";
          NIX_CFLAGS_COMPILE = "-Wno-error";
        };
        packages = {
          hydra-queue-runner = pkgs.callPackage hydra-queue-runner { };
          hydra-queue-builder = pkgs.callPackage hydra-queue-builder { };
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
        queue-builder = import ./darwin-builder-module.nix { inherit self; };
      };
      nixosModules = {
        queue-runner = import ./runner-module.nix { inherit self; };
        queue-builder = import ./linux-builder-module.nix { inherit self; };
      };
    };
}
