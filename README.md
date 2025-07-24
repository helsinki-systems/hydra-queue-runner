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

### Further Ideas

- Builder upload to s3 (saves a roundtrip), queue-runner can give a builder a presigned url for that (narhash already signed)
- Track additional build metrics for each step (cgroups)

## HTTP interface for debugging

- `GET /status`
- `GET /status/machines`
- `GET /status/jobsets`
- `GET /status/builds`
- `GET /status/steps`
- `GET /status/runnable`
- `GET /status/queues`
- `GET /status/queues/jobs`
- `GET /status/queues/scheduled`
- `PUT /build --json {"jobsetId": 1, "drv": "/nix/store/7ayly6qqhzk2x5ww3i3ibaxl69i5mqkg-hdf5-cpp-1.14.6.drv"}`
- `GET /metrics`

## queue runner config

```toml
hydraDataDir = "/var/lib/hydra"
dbUrl = "postgres://hydra@%2Frun%2Fpostgresql:5432/hydra"
maxDbConnections = 128
# valid options: SpeedFactorOnly, CpuCoreCountWithSpeedFactor, BogomipsWithSpeedFactor
dispatchTriggerTimerInS = 120
queueTriggerTimerInS = -1
machineSortFn = "SpeedFactorOnly"
remoteStoreAddr = "s3://my-bucket?region=eu-west-1"
signingKeyPath = "/run/secrets/hydra/signing_key"
# if queue is allowed to substitute
useSubstitutes = false
maxRetries = 5
retryInterval = 60
retryBackoff = 3.0
maxUnsupportedTimeInS = 60
stopQueueRunAfterInS = 60
```

## TODO list

TODO(conni2461): add list what currently works and what still needs to be done
- [ ] handle open todos in code
- [ ] upload nix logs? - not possible without using ffi
- [ ] more tests
- [ ] document inner workings
- [ ] github nix ci
- [ ] migrate hydra to 64bit timestamps
- [ ] status pages fixes
- [ ] ....
