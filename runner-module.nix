{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.services.queue-runner-dev;

  format = pkgs.formats.toml { };
  configFile = format.generate "queue-runner.toml" cfg.settings;
in
{
  options = {
    services.queue-runner-dev = {
      enable = lib.mkEnableOption (lib.mdDoc "QueueRunner");

      settings = lib.mkOption {
        type = lib.types.submodule { freeformType = format.type; };

        description = lib.mdDoc "Settings";
      };

      grpcPort = lib.mkOption {
        description = "Which grpc port this app should listen on";
        type = lib.types.port;
        default = 50051;
      };

      restPort = lib.mkOption {
        description = "Which rest port this app should listen on";
        type = lib.types.port;
        default = 8080;
      };

      mtls = lib.mkOption {
        description = "mtls options";
        default = null;
        type = lib.types.nullOr (
          lib.types.submodule {
            options = {
              serverCertPath = lib.mkOption {
                description = "Server certificate path";
                type = lib.types.path;
              };
              serverKeyPath = lib.mkOption {
                description = "Server key path";
                type = lib.types.path;
              };
              clientCaCertPath = lib.mkOption {
                description = "Client ca certificate path";
                type = lib.types.path;
              };
            };
          }
        );
      };

      package = lib.mkOption {
        type = lib.types.path;
        default = pkgs.callPackage ./. { };
      };
    };
  };

  config = lib.mkIf cfg.enable {
    services.queue-runner-dev.settings = builtins.mapAttrs (_: v: lib.mkDefault v) {
      hydra_log_dir = "/var/lib/hydra/logs";
      max_db_connections = 128;
      machine_sort_fn = "SpeedFactorOnly";
      use_substitute = false;
    };

    systemd.services.queue-runner-dev = {
      description = "queue-runner main service";

      requires = [ "nix-daemon.socket" ];
      after = [
        "network.target"
        "postgresql.service"
      ];
      wantedBy = [ "multi-user.target" ];

      environment = {
        NIX_REMOTE = "daemon";
        LIBEV_FLAGS = "4"; # go ahead and mandate epoll(2)
        RUST_BACKTRACE = "1";
      };

      # Note: it's important to set this for nix-store, because it wants to use
      # $HOME in order to use a temporary cache dir. bizarre failures will occur
      # otherwise
      environment.HOME = "/run/queue-runner";

      serviceConfig = {
        ExecStart = lib.escapeShellArgs (
          [
            "${cfg.package}/bin/queue-runner"
            "--rest-bind"
            "[::1]:${toString cfg.restPort}"
            "--grpc-bind"
            "[::1]:${toString cfg.grpcPort}"
            "--config-path"
            configFile
          ]
          ++ lib.optionals (cfg.mtls != null) [
            "--server-cert-path"
            cfg.mtls.serverCertPath
            "--server-key-path"
            cfg.mtls.serverKeyPath
            "--client-ca-cert-path"
            cfg.mtls.clientCaCertPath
          ]
        );

        User = "hydra-queue-runner";
        Group = "hydra";

        PrivateNetwork = false;
        SystemCallFilter = [
          "@system-service"
          "~@privileged"
          "~@resources"
        ];
        StateDirectory = "hydra";
        RuntimeDirectory = "queue-runner";

        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        PrivateMounts = true;
        RemoveIPC = true;
        UMask = "0077";

        CapabilityBoundingSet = "";
        NoNewPrivileges = true;

        ProtectKernelModules = true;
        SystemCallArchitectures = "native";
        ProtectKernelLogs = true;
        ProtectClock = true;

        RestrictAddressFamilies = "";

        LockPersonality = true;
        ProtectHostname = true;
        RestrictRealtime = true;
        MemoryDenyWriteExecute = true;
        PrivateUsers = true;
        RestrictNamespaces = true;
      };
    };

    users = {
      groups.hydra = { };
      users.hydra-queue-runner = {
        group = "hydra";
        isSystemUser = true;
      };
    };
  };
}
