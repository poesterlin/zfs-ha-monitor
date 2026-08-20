# zfs-ha-monitor

A small, read-only monitor that collects ZFS pool health and per-disk SMART data, then publishes it to Home Assistant over MQTT. It uses Home Assistant's MQTT discovery, so every entity appears automatically as a sensor or binary sensor — no manual YAML configuration required.

The binary is built as a fully static musl executable, meaning the same file runs on the Proxmox host, the development machine, or a test VM without needing to manage glibc compatibility.

It only ever runs read-only commands — `zpool status`, `zpool list`, and `smartctl` — so it never modifies your pools, disks, or data. A systemd unit and deploy script are included for running it continuously as a service on the host.

## Features

For each ZFS pool it publishes a binary sensor that flips to `problem` when the pool is not ONLINE, plus sensors for capacity, free space, used space, and fragmentation.

For each leaf disk it publishes SMART sensors for temperature, reallocated sectors, pending sectors, reported uncorrect, and offline uncorrectable counts. Each disk is surfaced as its own Home Assistant device, keyed by a stable unique id, so history and charts keep working across restarts.

All this works through Home Assistant MQTT discovery: the monitor publishes a config topic for each entity, and Home Assistant picks it up with no configuration on your side.

## Entities

Pool (`zfs/<node>/<pool>/*`):

| Entity | Kind | Notes |
| --- | --- | --- |
| `state` | binary_sensor | `no_problem` when ONLINE, else `problem` |
| `capacity` | sensor | % used |
| `fragmentation` | sensor | % |
| `free` | sensor | GB |
| `used` | sensor | GB |

Disk (`zfs/<node>/<pool>/disk/<name>/*`):

| Entity | Kind | Notes |
| --- | --- | --- |
| `temperature` | sensor | °C, device_class `temperature` |
| `reallocatedsectorct` | sensor | count |
| `reporteduncorrect` | sensor | count |
| `currentpendingsector` | sensor | count |
| `offlineuncorrectable` | sensor | count |

The SMART reliability counters intentionally omit `device_class`: `count` is not a valid Home Assistant sensor device class, so these are published as plain measurement sensors with `unit_of_measurement: count`.

## Prerequisites

- Rust with the `x86_64-unknown-linux-musl` target (for the static build).
- `musl` toolchain (`musl-gcc` or similar) on the build machine.
- An MQTT broker reachable from the host (Home Assistant's built-in broker works).
- `zpool` and `smartctl` on the host the binary runs on.

## Install & run

1. Copy `.env.example` to `.env` and configure your MQTT broker, credentials, and node name.
2. Build the static release binary:

   ```sh
   cargo build --release --target x86_64-unknown-linux-musl
   ```

3. Run it once against your pool to verify it publishes as expected. It reads from the pool read-only, so it is safe:

   ```sh
   export $(grep -vE '^#|^$' .env | xargs)
   ONCE=1 ./target/x86_64-unknown-linux-musl/release/zfs-ha-monitor
   ```

4. To run continuously as a service on a Proxmox host, use the bundled deploy script. It copies the binary, `.env`, and systemd unit to the host and enables the service:

   ```sh
   HOST=root@pve ./deploy.sh
   ```

## Configuration

All settings come from environment variables (see `.env.example`):

| Variable | Default | Description |
| --- | --- | --- |
| `MQTT_URL` | `mqtt://127.0.0.1:1883` | MQTT broker URL (may include `user:pass@host`) |
| `MQTT_USER` | empty | Explicit username (overrides any in URL) |
| `MQTT_PASS` | empty | Explicit password (overrides any in URL) |
| `MQTT_TOPIC` | `zfs` | Base topic: `<topic>/<node>/<pool>/...` |
| `NODE_NAME` | `pve` | Short host name used in entity ids / device names |
| `INTERVAL_SECS` | `60` | Seconds between poll-and-publish cycles |
| `ONCE` | `0` | `1` to publish a single snapshot and exit |
| `FAILURE_FIXTURE` | empty | Path to a YAML snapshot fixture to publish instead of live data |
| `MQTT_DISCOVERY_PREFIX` | `homeassistant` | Home Assistant MQTT discovery prefix |

## Testing a degraded pool

The binary can publish a loaded snapshot instead of collecting from a live pool. Set `FAILURE_FIXTURE` to a YAML fixture (matching the model in `src/model.rs`) to exercise a DEGRADED pool scenario without touching a real system. Unit tests cover parsing of the fixtures under `tests/fixtures/`.

## Directory layout

```
src/
  main.rs      entry point, collect loop
  config.rs    environment / .env configuration
  model.rs     Pool, Vdev, DiskSmart, Snapshot
  zfs.rs       zpool status/list parsing
  smart.rs     smartctl -A -j parsing
  mqtt.rs      MQTT publish + Home Assistant discovery
packaging/
  zfs-ha-monitor.service   systemd unit
deploy.sh                  build + deploy to a host
```
