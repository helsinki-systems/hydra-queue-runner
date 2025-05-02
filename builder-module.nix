{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.services.queue-builder-dev;
in
{
  options = {
    services.queue-builder-dev = {
      enable = lib.mkEnableOption (lib.mdDoc "QueueBuilder");

      queueRunnerAddr = lib.mkOption {
        description = "Queue Runner address to the grpc server";
        type = lib.types.singleLineStr;
      };

      pingInterval = lib.mkOption {
        description = "Interval in which pings are send to the runner";
        type = lib.types.int;
        default = 30;
      };

      speedFactor = lib.mkOption {
        description = "Additional Speed factor for this machine";
        type = lib.types.oneOf [
          lib.types.int
          lib.types.float
        ];
        default = 1;
      };

      mtls = lib.mkOption {
        description = "mtls options";
        default = null;
        type = lib.types.nullOr (
          lib.types.submodule {
            options = {
              serverRootCaCertPath = lib.mkOption {
                description = "Server root ca certificate path";
                type = lib.types.path;
              };
              clientCertPath = lib.mkOption {
                description = "Client certificate path";
                type = lib.types.path;
              };
              clientKeyPath = lib.mkOption {
                description = "Client key path";
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
    systemd.services.queue-builder-dev = {
      description = "queue-builder main service";

      requires = [ "nix-daemon.socket" ];
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      environment = {
        NIX_REMOTE = "daemon";
        LIBEV_FLAGS = "4"; # go ahead and mandate epoll(2)
        RUST_BACKTRACE = "1";
      };

      # Note: it's important to set this for nix-store, because it wants to use
      # $HOME in order to use a temporary cache dir. bizarre failures will occur
      # otherwise
      environment.HOME = "/run/queue-builder";

      serviceConfig = {
        Restart = "always";
        RestartSec = "5s";
        ExecStart = lib.escapeShellArgs (
          [
            "${cfg.package}/bin/builder"
            "--gateway-endpoint"
            cfg.queueRunnerAddr
            "--ping-interval"
            cfg.pingInterval
            "--speed-factor"
            cfg.speedFactor
          ]
          ++ lib.optionals (cfg.mtls != null) [
            "--server-root-ca-cert-path"
            cfg.mtls.serverRootCaCertPath
            "--client-cert-path"
            cfg.mtls.clientCertPath
            "--client-key-path"
            cfg.mtls.clientKeyPath
          ]
        );

        User = "hydra-queue-builder";
        Group = "hydra";

        PrivateNetwork = false;
        SystemCallFilter = [
          "@system-service"
          "~@privileged"
          "~@resources"
        ];
        RuntimeDirectory = "queue-builder";

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
      users.hydra-queue-builder = {
        group = "hydra";
        isSystemUser = true;
      };
    };
  };
}
