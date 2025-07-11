(import ./lib.nix) {
  name = "t00-simple";

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
          grpc.address = "[::]";
        };
        networking.firewall.allowedTCPPorts = [ 50051 ];
      };

    builder01 =
      { ... }:
      {
        imports = [ ../builder-module.nix ];

        services.queue-builder-dev = {
          enable = true;
          queueRunnerAddr = "http://runner:50051";
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

    builder01.wait_for_unit("queue-builder-dev.service")
    builder01.succeed("systemctl --failed | grep -q '^0 loaded'")  # Nothing failed

    # TODO: insert build and validate
  '';
}
