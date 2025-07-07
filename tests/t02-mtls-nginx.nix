(import ./lib.nix) {
  name = "t02-mtls-nginx";

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
        };

        services.nginx = {
          enable = true;
          virtualHosts.default = {
            default = true;
            extraConfig = ''
              ssl_client_certificate ${./certs/ca.crt};
              ssl_verify_depth 2;
              ssl_verify_client on;
            '';

            sslCertificate = ./certs/server.crt;
            sslCertificateKey = ./certs/server.key;
            onlySSL = true;

            locations."/".extraConfig = ''
              grpc_pass grpc://[::1]:50051;

              grpc_set_header Host $host;
              grpc_set_header X-Real-IP $remote_addr;
              grpc_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
              grpc_set_header X-Forwarded-Proto $scheme;

              grpc_set_header X-Client-DN $ssl_client_s_dn;
              grpc_set_header X-Client-Cert $ssl_client_escaped_cert;
            '';
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
    runner.wait_for_unit("nginx.service")
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
