{ pkgs, ... }:
{
  systemd.services.queue-runner-dev = {
    preStart = ''
      ${pkgs.postgresql_16}/bin/psql -U hydra hydra < ${pkgs.hydra.src}/src/sql/hydra.sql
    '';
  };

  services.postgresql = {
    enable = true;
    enableJIT = true;
    package = pkgs.postgresql_16;

    ensureDatabases = [ "hydra" ];
    ensureUsers = [
      {
        name = "hydra";
        ensureDBOwnership = true;
      }
    ];
    identMap = ''
      hydra-users hydra hydra
      hydra-users hydra-queue-runner hydra
      hydra-users hydra-www hydra
      hydra-users heb-dumps heb-dumps
    '';

    authentication = ''
      local hydra postgres ident
      local hydra all ident map=hydra-users
    '';
  };
}
