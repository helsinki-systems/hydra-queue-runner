{
  pkgs ?
    (builtins.getFlake (builtins.toString ./.)).inputs.nixpkgs.legacyPackages.${builtins.currentSystem},
  rustPlatform ? pkgs.rustPlatform,
  nix-gitignore ? pkgs.nix-gitignore,
  lib ? pkgs.lib,
  pkg-config ? pkgs.pkg-config,
  openssl ? pkgs.openssl,
  zlib ? pkgs.zlib,
  protobuf ? pkgs.protobuf,
  makeWrapper ? pkgs.makeWrapper,
  nixVersions ? pkgs.nixVersions,
  nlohmann_json ? pkgs.nlohmann_json,
  libsodium ? pkgs.libsodium,
  boost ? pkgs.boost,
}:
rustPlatform.buildRustPackage {
  name = "queue-runner";
  src = nix-gitignore.gitignoreSource [ ] (
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
  __structuredAttrs = true;
  strictDeps = true;

  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    protobuf
    makeWrapper
  ];
  buildInputs = [
    openssl
    zlib
    protobuf

    nixVersions.nix_2_29
    nlohmann_json
    libsodium
    boost
  ];

  postInstall = ''
    wrapProgram $out/bin/queue-runner \
      --prefix PATH : ${lib.makeBinPath [ nixVersions.nix_2_29 ]}
    wrapProgram $out/bin/builder \
      --prefix PATH : ${lib.makeBinPath [ nixVersions.nix_2_29 ]}
  '';

  meta = with lib; {
    description = "Hydra Queue-Runner implemented in rust";
    homepage = "https://github.com/helsinki-systems/queue-runner";
    license = with licenses; [ gpl3 ];
    maintainers = [ maintainers.conni2461 ];
    platforms = platforms.all;
  };
}
