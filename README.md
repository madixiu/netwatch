# NetWatch

NetWatch is an SNMP-based network monitoring system. It polls switches and routers, stores interface and health metrics in PostgreSQL, and shows them in a web dashboard. It does not install agents on monitored devices.

**Status: development.** Core polling, storage, rollups, scanning, and the dashboard are implemented. Production deployment and wider hardware validation are not finished. See [Known limitations](#known-limitations).

## Features

- **Polling over SNMP v2c and v3** (authPriv), with per-device credentials encrypted at rest with AES-GCM.
- **Interface metrics** from IF-MIB 64-bit counters: traffic in bits per second, packet rates, error and discard rates, and operational state.
- **Health metrics** for CPU and memory, from HOST-RESOURCES-MIB and vendor profiles where available.
- **Device reachability** through status polling and ICMP.
- **Network scanning.** Scan an IPv4 range from an allowlist, detect each device's vendor and model, and approve devices before polling starts. See [docs/scanning.md](docs/scanning.md).
- **Time-series storage** in daily-partitioned raw tables, with 1-minute, hourly, and daily rollups.
- **Retention.** Raw samples are dropped after the configured window, and old partitions are removed without slow `DELETE` statements.
- **Dashboard** built with React, Vite, and Tailwind, served by the API.
- **Local accounts** with `admin` and `viewer` roles.

## Architecture

```
  Browser ──HTTP──► netwatch-api (Axum) ──► PostgreSQL ◄── netwatch-poller ──SNMP/ICMP──► devices
                    serves web/dist                          scheduler, scan worker,
                    read API, admin API                      rollups, partitions, retention
```

- **netwatch-poller** is the only component that talks to devices. It runs the polling scheduler, the scan worker, rollups, partition management, and retention.
- **netwatch-api** reads from PostgreSQL and writes only device, credential, scan, and account records. It never contacts devices.
- **PostgreSQL** holds configuration, raw samples, rollups, poll status, and scan results.

The full design is in [NETWORK-MONITOR-v2.md](NETWORK-MONITOR-v2.md).

## Repository layout

```
netwatch/
├── Cargo.toml               Rust workspace
├── crates/
│   ├── core/                counter math, SNMP value types, OIDs, scan address logic
│   ├── crypto/              AES-GCM encryption, key loading, password hashing
│   ├── snmp/                SNMP client wrapper (v2c and v3)
│   ├── profiles/            vendor OID profiles (Cisco IOS, NX-OS, MikroTik)
│   ├── db/                  migrations, repositories, rollups, metrics queries
│   ├── poller/              netwatch-poller binary
│   └── api/                 netwatch-api, netwatch-migrate, netwatch-user binaries
├── web/                     React dashboard (Vite + Tailwind)
├── docs/
│   ├── runbook.md           operational notes and known gaps
│   └── scanning.md          scanning behaviour and limits
├── DEPLOY.md                deployment guide
└── NETWORK-MONITOR-v2.md    design document
```

## Quick start (development)

You need:

- Rust (current stable, via [rustup](https://rustup.rs))
- PostgreSQL 14 or newer
- Node.js 20.19 or newer (required by Vite 7)

1. Create a development database and set the connection string:

   ```sh
   export DATABASE_URL=postgres://netwatch:change-me@127.0.0.1/netwatch_dev
   ```

2. Create a development key. It must be 64 hexadecimal characters:

   ```sh
   export NETWATCH_KEY_HEX=$(openssl rand -hex 32)
   ```

   `NETWATCH_KEY_HEX` is for development only. For anything shared, use a key file. See [DEPLOY.md](DEPLOY.md).

3. Build the frontend:

   ```sh
   npm --prefix web ci
   npm --prefix web run build
   ```

4. In one terminal, start the poller. It applies migrations at startup:

   ```sh
   cargo run -p netwatch-poller
   ```

5. In a second terminal, create an admin account, then start the API:

   ```sh
   read -r -s -p 'New password: ' NETWATCH_PASSWORD; echo
   printf '%s\n' "$NETWATCH_PASSWORD" | cargo run -p netwatch-api --bin netwatch-user -- admin admin
   unset NETWATCH_PASSWORD
   cargo run -p netwatch-api --bin netwatch-api
   ```

6. Open `http://127.0.0.1:8080/` and sign in.

For frontend development with hot reload, run `npm --prefix web run dev`. It proxies API requests to `127.0.0.1:8080`, so the API must be running.

## Tests

```sh
cargo test
```

Some tests need a disposable PostgreSQL database. They are marked `#[ignore]`, so set `DATABASE_URL` to a database you can wipe, then run:

```sh
cargo test -- --ignored
```

Never point the ignored tests at a database with real data.

## Configuration

| Variable | Used by | Purpose |
|---|---|---|
| `DATABASE_URL` | poller, API | PostgreSQL connection string (required) |
| `NETWATCH_KEY_FILE` | poller, API | Path to a file containing the 64-character hex key. Preferred. |
| `NETWATCH_KEY_HEX` | poller, API | The key inline. Development only. |
| `NETWATCH_SCAN_ALLOW` | poller, API | Comma-separated IPv4 CIDRs that scans may target, e.g. `10.0.0.0/8`. Unset or empty disables scanning. |
| `NETWATCH_API_BIND` | API | Listen address. Default `127.0.0.1:8080`. |
| `NETWATCH_WEB_ROOT` | API | Directory with the built frontend. Default `web/dist`. |
| `NETWATCH_FAKE_SNMP` | poller | Uses simulated SNMP data. For testing only. |
| `RUST_LOG` | all | Log filter, for example `info` or `netwatch_poller=debug`. |

## Known limitations

- **Not production-validated.** The scan worker has been tested against one physical switch. Load, recovery, and long-running behaviour have not been validated.
- **Plain HTTP.** The API does not terminate TLS. Put it behind an HTTPS reverse proxy, or keep it on loopback. See [DEPLOY.md](DEPLOY.md).
- **Memory and CPU coverage varies.** CPU row selection and memory support have not been verified across all device models. Unsupported readings show as unavailable, not zero.
- **A stale status** means no recent poll result. It does not prove a device is down.
- **Deployment units are not yet in the repository.** The guide in [DEPLOY.md](DEPLOY.md) includes examples to adapt.
- **Single poller.** The design does not yet support multiple poller instances or remote pollers.

## Documentation

- [DEPLOY.md](DEPLOY.md): building, installing, and running in production
- [docs/runbook.md](docs/runbook.md): operations, security notes, and known gaps
- [docs/scanning.md](docs/scanning.md): how scanning works and its limits
- [NETWORK-MONITOR-v2.md](NETWORK-MONITOR-v2.md): architecture and design decisions

## Security

Passwords and session tokens travel over HTTP unless a proxy adds TLS. Keep the API on loopback, or protect it with HTTPS. Scanning sends SNMP probes to many addresses, so tell your network team before scanning any range. Only add ranges you are allowed to scan.

To report a security issue, contact the project owner directly. Do not open a public issue.

## License

Proprietary. All rights reserved.
