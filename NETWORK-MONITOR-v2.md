# NetWatch: Network Monitoring System (v2)

Revision 2 incorporates the design review. Changes from v1 are marked in the relevant sections and summarised in section 15.

## 1. Overview

NetWatch is an SNMP-based network monitoring system. It polls network devices, stores time-series metrics in PostgreSQL, and shows them in a web dashboard. It does not use agents on the monitored devices.

- **Backend:** Rust (Tokio, Axum, sqlx)
- **Frontend:** React, served by Axum as static files in phase 1
- **Database:** PostgreSQL 16+, daily-partitioned raw table, rollup tables
- **Protocols:** SNMPv2c and SNMPv3 (authPriv)

## 2. Goals

- Collect interface traffic, CPU, memory, and health metrics from switches and routers.
- Store history: raw samples for 14 days, daily rollups for a year.
- Show device status, port traffic graphs, and resource history.
- Add a device type by adding an OID profile, not by changing the poller.
- Never silently lose data. Every gap should be either recorded or explainable.

## 3. Non-goals (phase 1)

- Alerting (phase 3, built on rollup tables).
- Agents on devices.
- Config backup or any write to devices.
- Remote pollers. The poller runs where it can reach all devices. Remote pollers would need an ingest endpoint, which is deferred.

## 4. Scope by phase

| Phase | Scope | Status |
|---|---|---|
| 1 | Cisco IOS, NX-OS, MikroTik: polling, storage, dashboard | Current |
| 2 | Linux and Windows servers: host profiles, disks, processes | Planned |
| 3 | Alerting and notifications on rollup data | Planned |

Phase 1 milestone order is Cisco IOS, then NX-OS, then MikroTik (see section 13).

## 5. Architecture

```
            ┌────────────────────────────────────────┐
            │            React dashboard             │
            │   device list · port graphs · CPU/mem  │
            └───────────────────┬────────────────────┘
                                │ HTTPS (internal CA)
            ┌───────────────────▼────────────────────┐
            │     Axum API + static SPA (read-mostly)│
            │   /devices  /metrics  /status  /healthz│
            └───────────────────┬────────────────────┘
                                │
            ┌───────────────────▼────────────────────┐
            │            PostgreSQL                  │
            │  devices · credentials (encrypted)     │
            │  profiles · metrics_catalog · interfaces│
            │  raw_samples (daily partitions)        │
            │  rollup_1m · rollup_1h · rollup_1d     │
            │  rollup_state · poll_status            │
            │  poller_heartbeat                      │
            └───────────────────▲────────────────────┘
                                │ batched writes (COPY)
            ┌───────────────────┴────────────────────┐
            │       Poller engine (Rust, Tokio)      │
            │  scheduler · one task per device       │
            │  buffer · counter math · shutdown flush│
            │  /healthz + heartbeat                  │
            └──────┬──────────────────────┬──────────┘
                   │                      │
     ┌─────────────▼──────────┐ ┌─────────▼──────────────┐
     │  SNMP client           │ │  OID profiles          │
     │  v2c / v3 (auth, priv) │ │  logical metric → OID  │
     └─────────────┬──────────┘ │  + metrics()           │
                   │            └────────────────────────┘
     ┌─────────────▼──────────────────────────────────────┐
     │  Devices: Cisco IOS/NX-OS, MikroTik   (phase 1)    │
     │  Linux, Windows servers               (phase 2)    │
     └────────────────────────────────────────────────────┘

  Background jobs (in the poller process, or systemd timers)
  ┌──────────────────────────────────────────────────────────┐
  │ Partition job:  creates raw partitions for next 30 days  │
  │ Rollup job:     raw → 1m → 1h → 1d, watermark-driven     │
  │ Retention job:  drops raw partitions older than 14 days  │
  │ Catalog sync:   upserts profile metrics at startup       │
  └──────────────────────────────────────────────────────────┘

  Separate step, run before any service starts
  netwatch-migrate  (sqlx migrations; never run by the poller or API)
```

Key rules:

- Only the poller talks to devices. The API reads PostgreSQL and never calls SNMP.
- The poller writes metrics directly to PostgreSQL in batches.
- The API writes only device and credential records, through admin endpoints.
- Vendor differences live in the profile crate, not in the poller.
- Migrations run once, before services start. Services never migrate.

## 6. Flowchart (final design)

```
        ┌──────────────────────────┐
        │ Start poller process     │
        │ (after netwatch-migrate) │
        └────────────┬─────────────┘
                     ▼
        ┌──────────────────────────┐
        │ Load enabled devices     │◄──────────────────────────┐
        │ + credentials from DB    │                           │
        └────────────┬─────────────┘                           │
                     ▼                                         │
        ┌──────────────────────────┐                           │
        │ Scheduler: one task per  │   reload every few min;   │
        │ device                   │   disabled devices exit   │
        └────────────┬─────────────┘   on next loop iteration  │
                     ▼                                         │
        ┌──────────────────────────┐                           │
        │ Wait for next tick       │                           │
        └────────────┬─────────────┘                           │
                     ▼                                         │
        ┌──────────────────────────┐   previous poll still     │
        │ Overrun? ───────────────────running: skip tick,      │
        └────────────┬─────────────┘   write poll_status=skip  │
                     │ no                                      │
                     ▼                                         │
        ┌──────────────────────────┐   timeout / error         │
        │ SNMP GET / WALK          │── ───┐                    │
        │ (v2c or v3)              │      ▼                    │
        └────────────┬─────────────┘  ┌──────────────┐         │
                     │ ok             │ retry (max N)│         │
                     ▼                └──────┬───────┘         │
        ┌──────────────────────────┐         │ still failing   │
        │ Profile maps OIDs to     │         ▼                 │
        │ logical metrics          │  ┌──────────────┐         │
        └────────────┬─────────────┘  │ poll_status  │         │
                     ▼                │ = down / err │         │
        ┌──────────────────────────┐  └──────┬───────┘         │
        │ Counter math:            │         │                 │
        │ wrap / reset / first     │         │                 │
        │ sample; delta and dt     │         │                 │
        └────────────┬─────────────┘         │                 │
                     ▼                       │                 │
        ┌──────────────────────────┐         │                 │
        │ Buffer (per device and   │         │                 │
        │ global caps; drop oldest │         │                 │
        │ when full)               │         │                 │
        └────────────┬─────────────┘         │                 │
                     ▼                       │                 │
        ┌──────────────────────────┐         │                 │
        │ Flush: 500 rows or 5 s,  │         │                 │
        │ whichever first          │         │                 │
        │ COPY into raw_samples    │         │                 │
        │ + poll_status = ok       │         │                 │
        └────────────┬─────────────┘         │                 │
                     └───────────────────────┴─────────────────┘

  Shutdown (SIGTERM from systemd)
  ┌──────────────────────────────────────────────────────┐
  │ Stop scheduling → let in-flight polls finish (bounded)│
  │ → flush all buffers → write final heartbeat → exit    │
  └──────────────────────────────────────────────────────┘

  Background jobs
  ┌──────────────────────────────────────────────────────────┐
  │ Partition job: ensure partitions exist for next 30 days   │
  │ Rollup job: process completed buckets only (section 7)    │
  │ Retention: drop raw partitions older than 14 days         │
  │ Heartbeat: poller writes poller_heartbeat every 30 s      │
  └──────────────────────────────────────────────────────────┘

  Read path
  Dashboard → Axum API → table chosen by the rule in section 11
```

## 7. Data lifecycle

This section governs rollups, idempotency, late data, and retention. Everything else in the design depends on it.

### 7.1 Stages

```
device ──SNMP──► poller ──COPY──► raw_samples (14 d)
                                     │  rollup job, watermark-driven
                                     ▼
                              rollup_1m (90 d)
                                     │
                                     ▼
                              rollup_1h (400 d)
                                     │
                                     ▼
                              rollup_1d (1 y+)
```

Retention values are defaults set in configuration. Section 14 lists them as open decisions.

### 7.2 Counter versus gauge

Both the poller and the rollup job branch on `metrics_catalog.kind`.

- **Gauges** (CPU, memory, temperature): the poller stores the raw value. Rollups store `min`, `max`, `sum`, `count`, and `last`. Average is `sum / count`.
- **Counters** (traffic, error counters): the poller computes `delta` and `dt_seconds` at ingest and stores them in `raw_samples` alongside the raw value. Rollups store `sum_delta`, `sum_seconds`, `reset_count`, and `count`. Rate is `sum_delta / sum_seconds`.

Rolling up deltas, not rates, means a rate at any granularity is always correct. An hourly average of per-minute rates is wrong whenever a poll was late or a sample was skipped. Summing deltas and seconds is not affected by either.

### 7.3 Watermarks

A `rollup_state` table holds one row per granularity:

| Column | Meaning |
|---|---|
| `granularity` | `1m`, `1h`, or `1d` (primary key) |
| `watermark` | Start of the last bucket fully rolled up |
| `updated_at` | When the watermark last advanced |

The rollup job processes only buckets that have fully closed, meaning `bucket_end <= now()` minus a safety margin (for example 30 seconds for 1m). It never touches the in-progress bucket.

### 7.4 Idempotency and lookback

- Every rollup write is `INSERT ... ON CONFLICT (device_id, metric_id, interface_id, bucket_start) DO UPDATE`. Re-running any range produces the same rows.
- Each run re-processes the last 2 buckets behind the watermark. This catches a COPY from the poller that finished after the previous run.
- Watermarks advance only after the transaction commits.

### 7.5 Late data

Phase 1 has no backfill; the poller writes live. "Late" therefore means "the poller was slow or a flush was delayed." The lookback window in 7.4 covers delays up to two bucket lengths. Anything later is accepted as a permanent gap and marked in `poll_status`. The rollup job does not try to correct it.

### 7.6 Partitions

- The partition job runs nightly and ensures daily partitions exist for the next 30 days.
- A `raw_samples_default` partition catches any row whose day has no partition. A non-empty default partition triggers an alert in the logs, so the job's failure is visible.
- Retention drops whole partitions older than 14 days. It never runs `DELETE` on raw data.

### 7.7 Schema sketch

```sql
-- raw, partitioned by day
CREATE TABLE raw_samples (
    device_id     bigint      NOT NULL,
    metric_id     int         NOT NULL,
    interface_id  bigint,                 -- NULL for device-level metrics
    ts            timestamptz NOT NULL,
    value         double precision,       -- raw gauge value or raw counter
    delta         double precision,       -- counters only, after wrap correction
    dt_seconds    double precision,       -- counters only, actual elapsed time
    PRIMARY KEY (device_id, metric_id, COALESCE(interface_id, 0), ts)
) PARTITION BY RANGE (ts);

-- rollup (same shape for 1m, 1h, 1d)
CREATE TABLE rollup_1h (
    device_id     bigint      NOT NULL,
    metric_id     int         NOT NULL,
    interface_id  bigint      NOT NULL DEFAULT 0,
    bucket_start  timestamptz NOT NULL,
    count         int         NOT NULL,
    min_value     double precision,
    max_value     double precision,
    sum_value     double precision,
    last_value    double precision,
    sum_delta     double precision,
    sum_seconds   double precision,
    reset_count   int         NOT NULL DEFAULT 0,
    PRIMARY KEY (device_id, metric_id, interface_id, bucket_start)
);
```

Note: the primary key on `raw_samples` includes a COALESCE expression, which PostgreSQL does not allow in a partitioned table's primary key. Use `interface_id bigint NOT NULL DEFAULT 0` instead, with 0 meaning device-level. Use the same convention in the rollup tables. The sketch shows the intent; the migration will use the workable form.

Rollup from a lower granularity uses the same columns: `count` sums, `min` and `max` take min and max, `sum_value` and `sum_delta` and `sum_seconds` sum, `last_value` takes the last by time, and `reset_count` sums. This is why every rollup row carries `count`, so re-rolling 1m into 1h is weighted correctly.

## 8. Data model

| Table | Purpose | Notes |
|---|---|---|
| `devices` | Inventory: name, management IP, vendor, model, profile, `profile_version`, SNMP version, poll interval, enabled | `profile_version` records which OID mapping produced the samples |
| `credentials` | SNMP v2c community or v3 user, auth and privacy protocol | Secrets encrypted with AES-GCM; key kept outside the database |
| `profiles` | Profile name, version, detection rules | Loaded from the profile crate at startup |
| `interfaces` | Per-device ports: ifIndex, name, speed, `first_seen`, `removed_at` | Soft delete via `removed_at`; never hard-delete |
| `metrics_catalog` | Logical metric definitions: name, unit, `kind` (gauge or counter), counter width | Upserted from each profile's `metrics()` at startup |
| `raw_samples` | Raw value, delta, dt_seconds per sample | Daily partitions; 14-day retention |
| `rollup_1m`, `rollup_1h`, `rollup_1d` | Count, min, max, sum, last, sum_delta, sum_seconds, reset_count | Built by the rollup job |
| `rollup_state` | Watermark per granularity | Advances only after commit |
| `poll_status` | device_id, timestamp, status (ok, timeout, error, skipped, down), duration, error text | Used for up/down and for explaining gaps |
| `poller_heartbeat` | Poller instance, last beat timestamp, version | One row per poller instance |

Interfaces are soft-deleted. When an interface disappears, `removed_at` is set. Historical samples keep a valid reference, and the dashboard can show that a port existed between two dates. A renamed interface keeps its row, since `ifIndex` is stable. A new interface gets a new row with `first_seen` set.

## 9. Polling behaviour

- Default interval: 60 seconds per device, configurable per device.
- Traffic uses 64-bit counters (`ifHCInOctets`, `ifHCOutOctets`). The profile declares the counter width for each metric. Some vendors expose only 32-bit counters on some interfaces.
- CPU and memory come from the vendor profile or HOST-RESOURCES-MIB.
- **Rate uses actual timestamps.** `dt_seconds` is the real time between two successful polls. If a poll ran 5 seconds late, the rate divides by 65 seconds, not 60.
- **Wrap versus reset.** A value going past the counter maximum is a wrap, and the delta is corrected by adding the maximum. A value going down without wrapping is a reset, typically a reboot. A reset produces no delta and starts a new baseline. The sample is counted in `reset_count`.
- **First sample.** No previous value means no delta. The sample stores the raw counter and a null delta. The poller never emits a rate it cannot justify.
- **Overrun.** If the previous poll for a device is still running when the next tick arrives, the tick is skipped and `poll_status` records `skipped`. Skipping avoids piling requests onto a slow device. Because deltas use real timestamps, the next successful poll still produces a correct rate.
- Timeout and retry per device. Repeated failures mark the device down but never stop other devices.
- Writes are batched per device through the buffer (section 10.1), flushed with `COPY`.

Estimated load for 100 devices at 60 seconds: about 5,000 to 10,000 rows per minute, roughly 7 to 14 million rows per day.

### 9.1 Buffer and backpressure

- **Caps:** each device buffer holds at most 5,000 rows, and the global buffer holds at most 200,000 rows.
- **Drop policy:** when a cap is reached, the oldest rows are dropped first, and a counter of dropped rows is logged and written to `poll_status`. Dropping old monitoring data is preferable to running out of memory.
- **Flush triggers:** 500 rows or 5 seconds since the last flush, whichever comes first, per device.
- **Shutdown:** on SIGTERM the poller stops scheduling new polls, waits up to 20 seconds for in-flight polls, flushes all buffers, writes a final heartbeat, and exits. systemd's `TimeoutStopSec` is set to 30 seconds to allow this.
- **Database down:** the buffer keeps accepting rows up to its caps, retries flushes with backoff, and logs each failure. Drops are counted as above.

## 10. Credentials and security

- SNMP v3 credentials and v2c community strings are encrypted with AES-GCM before they reach the database.
- The encryption key is read at startup from a file (default `/etc/netwatch/secret.key`) or an environment variable. It is never stored in the database or the repository.
- Key file permissions are `0600`, owned by the poller's service user. The runbook documents the check.
- **Key rotation (manual, phase 1):** generate a new key, re-encrypt all credential rows with a one-off maintenance command that reads with the old key and writes with the new, verify a sample of devices still poll, then retire the old key. Write this procedure in `docs/runbook.md` before the first production use.
- **Key loss is unrecoverable.** If the key is lost, every credential must be re-entered. The runbook states this so nobody is surprised.
- The API serves HTTPS using certificates from the internal CA.
- Dashboard login is required. Admin actions (adding devices and credentials) require the admin role.
- Remote-site devices are reached over the existing OpenVPN tunnels, so no SNMP port is exposed outside the network.

## 11. API (phase 1)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/devices` | List devices with current status |
| POST | `/api/devices` | Add a device (admin) |
| GET | `/api/devices/{id}` | Device detail and interfaces |
| GET | `/api/metrics/traffic?device=&if=&from=&to=` | Interface traffic series |
| GET | `/api/metrics/resources?device=&from=&to=` | CPU and memory series |
| GET | `/api/status` | Fleet summary (up, down, error, skipped counts) |
| GET | `/healthz` | API liveness and database check |

### 11.1 Table-selection rule (single source of truth)

This rule is implemented in one function, `choose_source(range, granularity_hint)`, in `db::queries`, with a comment block that repeats it.

- If the requested range is 6 hours or less, read `raw_samples` and compute rates from `delta` and `dt_seconds`.
- If the range is longer than 6 hours, read the rollup table whose bucket size fits the range: `rollup_1m` up to 48 hours, `rollup_1h` up to 60 days, `rollup_1d` beyond that.
- Ranges are never stitched across raw and rollups. This avoids the jumps that appear when two sources meet.

The boundary is a constant that can be changed in one place if the dashboard needs a different cut-over.

## 12. Deployment

- **Services:** `netwatch-poller` and `netwatch-api`, each under systemd.
- **Migration:** `netwatch-migrate` is a separate binary. Deployment runs it once before starting services. It is wired either as a oneshot unit that the services `Requires=` and `After=`, or as `ExecStartPre=` on the services. Migrations never run inside the poller or API.
- **Partition job:** runs inside the poller as a scheduled task, with a systemd timer as a fallback that calls the same command.
- **PostgreSQL:** on a dedicated host or VM, with nightly backups and point-in-time recovery enabled.
- **Poller host:** must reach all devices, including remote sites through OpenVPN.
- **API host:** behind HTTPS, on the same host as the poller or a separate one. Axum serves the React build, so there is one service and one TLS certificate.

Deployment files live in `deploy/systemd/`: `netwatch-migrate.service`, `netwatch-poller.service`, `netwatch-api.service`, and the partition timer if used.

## 13. Roadmap

Phase 1 is built as a vertical slice before the vendor surface grows.

| Milestone | Deliverable |
|---|---|
| M1 | Spike and vertical slice on **Cisco IOS first**: poll, store, roll up, and show one device on the dashboard. Test SNMPv3 authPriv in the chosen crate here. |
| M1b | Add **NX-OS** profile against the same pipeline. |
| M1c | Add **MikroTik** profile. Expect the most profile work here. |
| M2 | Core engine hardening: buffer caps, shutdown flush, heartbeat, `/healthz`. Prerequisite: section 7 (data lifecycle) is implemented and tested. |
| M3 | Storage: schema migrations, partition job, rollup job with watermarks, retention |
| M4 | Dashboard: API with the table-selection rule, React views for traffic, CPU, memory |
| M5 | Hardening: HTTPS via internal CA, simulator-based integration tests, runbook, key rotation procedure |
| M6 | Phase 1 release |
| Phase 2 | Server profiles (Linux, Windows) and host metrics |
| Phase 3 | Alerting on rollup data |

The data lifecycle implementation, including the partition job, is a gate for M3 and should not start until section 7 is reviewed.

## 14. Open decisions

Resolved in this revision:

- Raw retention: 14 days (default, configurable).
- Rollup retention: 90 days for 1m, 400 days for 1h, one year or more for 1d.
- Table-selection rule: clamp at 6 hours, no stitching.
- Rollup strategy: sum deltas and seconds for counters, store count in every rollup.

Still open:

- Whether the dashboard is reachable outside the internal network.
- Authentication method: local accounts or directory integration.
- Whether remote pollers are needed later (would add an ingest endpoint).
- **SNMP Rust crate.** Candidates are `snmp`, `async-snmp`, `rusty-snmp`, or shelling out to net-snmp. Choose based on SNMPv3 authPriv coverage. Test this in M1 before committing.
- **Rollup watermark strategy.** Decided in principle: a `rollup_state` table with per-granularity watermarks, as in section 7.3. Confirm during M3.
- **Partition management.** Hand-rolled partition job versus pg_partman. Default is hand-rolled; revisit if partition handling grows.

## 15. Changes from v1

- Added section 7, the data lifecycle: stages, counter and gauge branching, watermarks, idempotency, lookback, late data, partitions, and a schema sketch.
- Section 5: added the partition, rollup, retention, and catalog jobs; the heartbeat and `/healthz`; and the rule that migrations run before services.
- Section 6: added overrun skip, shutdown flush, and the buffer flush trigger.
- Section 8: added `rollup_state`, `profiles`, interface soft delete, `profile_version`, `poller_heartbeat`, and richer rollup columns.
- Section 9: added timestamp-based rates, wrap versus reset handling, first-sample behaviour, overrun handling, and the buffer and backpressure policy (9.1).
- Section 10: added key file permissions, key rotation, and key-loss recovery.
- Section 11: replaced the vague table-selection rule with the single clamped rule (11.1).
- Section 12: added `netwatch-migrate` and the partition job.
- Section 13: reordered M1 to Cisco IOS, then NX-OS, then MikroTik; added the data lifecycle as an M3 gate.
- Section 14: resolved three decisions and added the three open items from the review.
