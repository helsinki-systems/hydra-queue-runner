# Hydra Queue-Runner

C++ port to Rust and no longer using remote `nix-store --write --serve`
protocol but communication over GRPC. Implementation is still **work in
progress** and is currently being tested. Also not yet feature complete.

Currently only interfacing with nix via nix binaries.

## Architecture

This Queue-Runner implementation works based on a Client/Server Architecture,
independent of the nix daemon protocol. The communication is done via grpc
between clients (builder/machines) and the server (queue-runner). The clients
join the server with a join message and open a bi-directional channel which is
used the send commands, this allows us the flexibility that the clients do not
need a public IP.

Message workflow (see: `proto/v1/streaming.proto`)
- Client sends a join message (hostname, nix systems, cpu_count, ...) inside a
  new bidirectional channel and will continue to send heartbeat/ping messages
  (load, pressure, memory usage).
- Server sends a build .drv message with requisites and build options.
- Client pulls all requisites that are not already present (separate rpc call), and afterwards builds the drv and streams back the logs (separate rpc call)
- Once the build is completed (succeeded or not) the client streams outputs (only if the build succeeded) and returns a build message containing metadata like nix-support information and import/build time.
- Queue runner writes results to database, signs and uploads output paths to s3

### Furthor Ideas

- Builder upload to s3 (saves a roundtrip), queue-runner can give a builder a presigned url for that (narhash already signed)
- Track additional build metrics for each step (cgroups)

## HTTP interface for debugging

- `GET /status`
- `GET /status/jobsets`
- `GET /status/builds`
- `GET /status/steps`
- `GET /status/runnable`
- `GET /status/queues`
- `PUT /build --json {"jobset_id": 1, "drv": "/nix/store/7ayly6qqhzk2x5ww3i3ibaxl69i5mqkg-hdf5-cpp-1.14.6.drv"}`
- `GET /metrics`

## queue runner config

```toml
hydra_log_dir = "/var/lib/hydra/logs"
db_url = "postgres://hydra@%2Frun%2Fpostgresql:5432/hydra"
max_db_connections = 128
# valid options: SpeedFactorOnly, CpuCoreCountWithSpeedFactor, BogomipsWithSpeedFactor
machine_sort_fn = "SpeedFactorOnly"
remote_store_addr = "s3://my-bucket?region=eu-west-1"
signing_key_path = "/run/secrets/hydra/signing_key"
# if queue is allowed to substitute
use_substitute = false
```

## TODO list

TODO(conni2461): add list what currently works and what still needs to be done
- [ ] handle open todos in code
- [ ] upload nix logs? - not possible without using ffi
- [ ] extract hydra build products?
- [ ] handle unsupported steps
- [ ] more tests
- [ ] document inner workings
- [ ] maybe drop runnable vec and move into Queues
- [ ] github nix ci
- [ ] migrate hydra to 64bit timestamps
- [ ] ....
