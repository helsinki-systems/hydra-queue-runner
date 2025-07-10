{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.services.queue-runner-dev;

  format = pkgs.formats.toml { };
in
{
  options = {
    services.queue-runner-dev = {
      enable = lib.mkEnableOption "QueueRunner";

      settings = lib.mkOption {
        description = "Reloadable settings for queue runner";
        type = lib.types.submodule {
          options = {
            hydraLogDir = lib.mkOption {
              description = "Hydra log directory";
              type = lib.types.path;
              default = "/var/lib/hydra/build-logs";
            };
            dbUrl = lib.mkOption {
              description = "Postgresql database url";
              type = lib.types.singleLineStr;
              default = "postgres://hydra@%2Frun%2Fpostgresql:5432/hydra";
            };
            maxDbConnections = lib.mkOption {
              description = "Postgresql maximum db connections";
              type = lib.types.ints.positive;
              default = 128;
            };
            machineSortFn = lib.mkOption {
              description = "Function name for sorting machines";
              type = lib.types.enum [
                "SpeedFactorOnly"
                "CpuCoreCountWithSpeedFactor"
                "BogomipsWithSpeedFactor"
              ];
              default = "SpeedFactorOnly";
            };
            dispatchTriggerTimerInS = lib.mkOption {
              description = "Timer for triggering dispatch in an interval in seconds. Setting this to a value <= 0 will disable this timer and only trigger the dispatcher if queue changes happend.";
              type = lib.types.int;
              default = 120;
            };
            remoteStoreAddr = lib.mkOption {
              description = "Remote store address";
              type = lib.types.nullOr lib.types.singleLineStr;
              default = null;
            };
            signingKeyPath = lib.mkOption {
              description = "Signing key path";
              type = lib.types.nullOr lib.types.path;
              default = null;
            };
            useSubstitutes = lib.mkOption {
              description = "Use substitution for paths";
              type = lib.types.bool;
              default = false;
            };
            rootsDir = lib.mkOption {
              description = "Gcroots directory, defaults to /nix/var/nix/gcroots/per-user/$LOGNAME/hydra-roots";
              type = lib.types.nullOr lib.types.path;
              default = null;
            };
          };
        };
        default = { };
      };

      grpcAddress = lib.mkOption {
        type = lib.types.singleLineStr;
        default = "[::1]";
        description = "The IP address the grpc listener should bound to";
      };

      grpcPort = lib.mkOption {
        description = "Which grpc port this app should listen on";
        type = lib.types.port;
        default = 50051;
      };

      restAddress = lib.mkOption {
        type = lib.types.singleLineStr;
        default = "[::1]";
        description = "The IP address the rest listener should bound to";
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
        type = lib.types.package;
        default = pkgs.callPackage ./. { };
      };
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.queue-runner-dev = {
      description = "queue-runner main service";

      requires = [ "nix-daemon.socket" ];
      after = [
        "network.target"
        "postgresql.service"
      ];
      wantedBy = [ "multi-user.target" ];
      reloadTriggers = [ config.environment.etc."hydra/queue-runner.toml".source ];

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
        Type = "notify";
        Restart = "always";
        RestartSec = "5s";

        ExecStart = lib.escapeShellArgs (
          [
            "${cfg.package}/bin/queue-runner"
            "--rest-bind"
            "${cfg.restAddress}:${toString cfg.restPort}"
            "--grpc-bind"
            "${cfg.grpcAddress}:${toString cfg.grpcPort}"
            "--config-path"
            "/etc/hydra/queue-runner.toml"
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
        ExecReload = "${pkgs.util-linux}/bin/kill -HUP $MAINPID";

        User = "hydra-queue-runner";
        Group = "hydra";

        PrivateNetwork = false;
        SystemCallFilter = [
          "@system-service"
          "~@privileged"
          "~@resources"
        ];
        StateDirectory = [ "hydra/queue-runner" ];
        StateDirectoryMode = "0700";
        ReadWritePaths = [
          "/nix/var/nix/gcroots/"
          "/run/postgresql/.s.PGSQL.${toString config.services.postgresql.port}"
          "/nix/var/nix/daemon-socket/socket"
          "/var/lib/hydra/build-logs/"
        ];
        ReadOnlyPaths = [ "/nix/" ];
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

    environment.etc."hydra/queue-runner.toml".source = format.generate "queue-runner.toml" (
      lib.filterAttrsRecursive (_: v: v != null) cfg.settings
    );
    systemd.tmpfiles.rules = [
      "d /nix/var/nix/gcroots/per-user/hydra-queue-runner 0755 hydra-queue-runner hydra -"
      "d /var/lib/hydra/build-logs/ 0755 hydra-queue-runner hydra -"
    ];

    users = {
      groups.hydra = { };
      users.hydra-queue-runner = {
        group = "hydra";
        isSystemUser = true;
      };
    };
  };
}
