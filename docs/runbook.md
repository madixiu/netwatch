# Development runbook

NetWatch requires PostgreSQL, Rust, Node.js, and npm. It includes discovery,
device monitoring, and a metrics dashboard; production deployment and extended
validation remain unfinished.

## Prepare the application

1. Set `DATABASE_URL` for an application database. The poller currently applies
   the database migrations at startup; start it before the API. To migrate
   without starting device polling, run
   `cargo run -p netwatch-api --bin netwatch-migrate` before starting the API.
2. Set `NETWATCH_KEY_FILE` to a file containing a 64-character hexadecimal
   encryption key. The file must not be accessible to group or other users.
   The API and poller must use the same key. `NETWATCH_KEY_HEX` is available for
   development only; environment values can be visible to other processes.
3. Configure `NETWATCH_SCAN_ALLOW` on both services, for example
   `10.0.0.0/8,192.168.1.0/24`. An empty or missing allowlist disables scanning.
   Tell the network team before scanning. Do not scan networks without approval.
4. Install and build the frontend from the repository root:

   ```sh
   npm --prefix web ci
   npm --prefix web run build
   ```

5. Start `cargo run -p netwatch-poller`, then create a local account. The
   account command reads its password from standard input rather than command
   arguments. For example, in Bash:

   ```sh
   read -r -s -p 'New password: ' NETWATCH_PASSWORD; echo
   printf '%s\n' "$NETWATCH_PASSWORD" | cargo run -p netwatch-api --bin netwatch-user -- admin admin
   unset NETWATCH_PASSWORD
   ```

   The final two arguments are the username and role (`admin` or `viewer`).
   Passwords must be between 12 and 1024 bytes. Run this command only on a
   trusted host; anyone with database write access can create accounts.
6. Start `cargo run -p netwatch-api --bin netwatch-api`, and open
   `http://127.0.0.1:8080/` on the same host. The server serves `web/dist` by
   default; use `NETWATCH_WEB_ROOT` if its working directory differs.

For frontend development, `npm --prefix web run dev` starts Vite and proxies
API requests to `127.0.0.1:8080`. Rebuild the frontend before using the API's
static-file server.

## Security

The API listens on loopback by default. HTTP transmits passwords and session
tokens in plaintext. Keep the API and Vite bound to loopback unless traffic is
protected by a trusted encrypted tunnel or HTTPS proxy. `NETWATCH_API_BIND`
changes the listen address. Sessions are bearer tokens stored in the browser's
session storage; they expire after eight hours and are revoked on sign-out.

Login attempts are limited in each API process by the observed client IP and
username. Behind a reverse proxy the observed IP is the proxy's, so the IP limit
is shared by its users. The API does not trust forwarded IP headers. Apply
additional rate limiting at the proxy, especially if multiple API processes are
deployed.

## Known gaps

Deployment units and extended physical-device, load, and recovery validation
remain unfinished. The scan worker has been tested on one physical switch;
this is not production validation. See [scanning.md](scanning.md) for scanning
details.

The dashboard displays polled devices, traffic in bits per second, and available
CPU and memory percentages. A stale status means no recent poll status, not
proof that the device is down. Memory requires a single RAM row in
HOST-RESOURCES-MIB; unsupported or ambiguous memory is shown as unavailable.
CPU row selection and memory support have not been verified across device
models.

The interface view includes operational state, utilization, packet rates, and
error/discard rates when the corresponding IF-MIB counters are supported.
Utilization uses the latest discovered interface speed. Rates require
valid counter continuity; unavailable readings are not zero. Historical
operational state shows the last observed state in each rollup bucket.

ICMP probes require the system `ping` utility and permission to send ICMP.
They run separately from SNMP, with up to 64 concurrent probes. A failed ping
can mean filtering rather than an outage. SNMP response time measures one
sysUpTime request, including local queuing and retries; an SNMP failure can
also mean authentication failure.
Probe history is retained for 90 days; the dashboard shows the latest result
and marks old results stale.

Hardware inventory is refreshed approximately every five minutes using
standard ENTITY-MIB, ENTITY-SENSOR-MIB, and ENTITY-STATE-MIB only. It displays
temperature sensors and fan/power operational states when exposed by the
device. Sensor "ok" means the reading is obtainable, not that temperature is
within a safe threshold. Vendor-specific hardware MIBs are not implemented;
hardware data is a current snapshot, not historical time series. These new
readings have not been validated on physical hardware.

Minute, hourly, and daily rollups are built from closed buckets; the two most
recent buckets are recomputed for late data. Raw data is retained for at least
14 days, minute rollups for at least 90 days, and hourly rollups for at least
400 days. Retention pauses or keeps extra history when the next rollup watermark
has not progressed far enough. Daily rollups are not automatically deleted.
Writes later than the two-bucket lookback are not backfilled automatically.
The poller logs a warning when a buffered batch is more than two minutes old;
inspect historical rollups after delayed writes. This is a conservative warning,
not automatic repair or a guarantee of detecting every late write.
Ranges of up to 6 hours read raw data; longer ranges select minute (up to 48
hours), hourly (up to 60 days), or daily rollups. Rollup queries exclude partial
boundary buckets, and the dashboard shows the processing watermark.

The poller refreshes interface inventory at startup and every five minutes.
If discovery fails, it retries after 30 seconds or at the next poll, whichever
is later, and retains its previous snapshot. Until an interface is discovered,
its samples are skipped. Existing samples written before interface discovery was
implemented may contain raw ifIndex values rather than interface row IDs.
Existing interface samples without verified mapping provenance are preserved
but excluded from traffic graphs and rollups, including samples written before
the provenance migration. Device-level samples remain available. Removed
interfaces remain selectable for historical data; `first_seen` for interfaces
created before that column was added reflects the migration time.

The poller requests `sysUpTime.0` and `ifCounterDiscontinuityTime` alongside
counter polls. It calculates deltas using the original integer counters; the
stored raw `value` remains floating point and may be rounded for large counters.
The first sample after a poller restart, unavailable continuity metadata, clock
disagreement, and ambiguous counter decreases produce no delta or rate interval.
A 64-bit decrease is recorded as a reset and starts a new baseline. A 32-bit
decrease from the lower half of its range is also recorded as a reset; a
possible 32-bit wrap is accepted only when the reported link speed makes it
plausible. Reset samples have no delta or rate interval.
Continuity checks are heuristic, not proof against every device reset. This
counter path has been tested with simulated SNMP, not the physical switch.
