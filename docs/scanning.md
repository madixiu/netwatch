# Scanning development status

The poller reads `NETWATCH_SCAN_ALLOW` as a comma-separated list of IPv4 CIDRs.
An unset or empty allowlist disables scanning. The worker validates each queued
range again before sending probes, limits scans to /22 or smaller, and excludes
network and broadcast addresses except for /31 and /32.

The poller executes one scan at a time, with a shared per-process limit of 64
in-flight SNMP operations across scans and device polling. Scans use one
credential profile and do not try a default community. Tell your network team
before scanning a real network.

The Scan page supports SNMP v2c and SNMPv3 authPriv credential profiles. Only
administrators can create profiles, start or cancel scans, and approve or reject
results. Viewers can inspect scans and results. Approved devices are picked up
by the poller without restarting it.

The worker's probe logic has mock tests, but has not been validated against real
devices or an SNMP simulator. The API and poller must use the same key and scan
allowlist; a mismatch can cause a queued scan to fail when the poller claims it.
