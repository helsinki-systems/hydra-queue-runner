(import ./lib.nix) {
  name = "t01-mtls";

  nodes = {
    runner =
      { inputs, ... }:
      {
        imports = [
          inputs.self.outputs.nixosModules.queue-runner
          ./hydra_postgresql.nix
        ];

        services.queue-runner-dev = {
          enable = true;
          grpc.address = "[::]";
          mtls = {
            clientCaCertPath = "${./certs/ca.crt}";
            serverCertPath = "${./certs/server.crt}";
            serverKeyPath = "${./certs/server.key}";
          };
        };
        networking.firewall.allowedTCPPorts = [ 50051 ];
      };

    builder01 =
      { inputs, ... }:
      {
        imports = [
          inputs.self.outputs.nixosModules.queue-builder
        ];

        services.queue-builder-dev = {
          enable = true;
          queueRunnerAddr = "https://runner:50051";
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

    builder01.wait_for_unit("queue-builder-dev.service")
    builder01.succeed("systemctl --failed | grep -q '^0 loaded'")  # Nothing failed

    # TODO: insert build and validate
  '';
}
