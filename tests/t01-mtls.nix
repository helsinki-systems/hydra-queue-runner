(import ./lib.nix) {
  name = "t01-mtls";

  nodes = {
    runner =
      { ... }:
      {
        imports = [
          ../runner-module.nix
          ./hydra_postgresql.nix
        ];

        services.queue-runner-dev = {
          enable = true;
          settings = {
            db_url = "postgres://hydra@%2Frun%2Fpostgresql:5432/hydra";
          };
          mtls = {
            clientCaCertPath = "${./certs/ca.crt}";
            serverCertPath = "${./certs/server.crt}";
            serverKeyPath = "${./certs/server.key}";
          };
        };
      };

    builder01 =
      { ... }:
      {
        imports = [ ../builder-module.nix ];

        services.queue-builder-dev = {
          enable = true;
          queueRunnerAddr = "https://runner";
          mtls = {
            serverRootCaCertPath = "${./certs/server.crt}";
            clientCertPath = "${./certs/client.crt}";
            clientKeyPath = "${./certs/client.key}";
            domainName = "localhost";
          };
        };
      };
  };

  testScript = ''
    start_all()

    runner.wait_for_unit("multi-user.target")
    runner.wait_for_unit("queue-runner-dev.service")
    runner.wait_for_open_port(50051)
    runner.wait_for_open_port(8080)
    runner.succeed("curl -sSfL 'http://[::1]:8080/metrics'")
    runner.succeed("systemctl --failed | grep -q '^0 loaded'")  # Nothing failed

    # restart builder once server is up
    builder01.succeed("systemctl restart queue-builder-dev.service")
    builder01.wait_for_unit("queue-builder-dev.service")
    builder01.succeed("systemctl --failed | grep -q '^0 loaded'")  # Nothing failed

    # TODO: insert build and validate
  '';
}
