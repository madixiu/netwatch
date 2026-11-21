-- Network scanning: reusable credential profiles, scan jobs, and scan results.
-- See SCANNING.md sections 5 and 7.

CREATE TABLE credential_profiles (
    id                        BIGSERIAL PRIMARY KEY,
    name                      TEXT NOT NULL UNIQUE,
    snmp_version              SMALLINT NOT NULL CHECK (snmp_version IN (2, 3)),
    snmp_v2c_community_enc    TEXT,
    snmp_v3_username          TEXT,
    snmp_v3_auth_proto        TEXT,
    snmp_v3_auth_secret_enc   TEXT,
    snmp_v3_priv_proto        TEXT,
    snmp_v3_priv_secret_enc   TEXT,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (snmp_version = 2 AND snmp_v2c_community_enc IS NOT NULL) OR
        (snmp_version = 3 AND snmp_v3_username IS NOT NULL)
    )
);

CREATE TYPE scan_status AS ENUM ('queued', 'running', 'done', 'cancelled', 'failed', 'interrupted');

CREATE TABLE scan_jobs (
    id                  BIGSERIAL PRIMARY KEY,
    cidr                CIDR NOT NULL,
    credential_profile_id BIGINT NOT NULL REFERENCES credential_profiles(id),
    status              scan_status NOT NULL DEFAULT 'queued',
    requested_by        TEXT NOT NULL,
    addresses_total     INTEGER NOT NULL DEFAULT 0,
    addresses_done      INTEGER NOT NULL DEFAULT 0,
    found_count         INTEGER NOT NULL DEFAULT 0,
    cancel_requested    BOOLEAN NOT NULL DEFAULT FALSE,
    error               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at          TIMESTAMPTZ,
    finished_at         TIMESTAMPTZ
);
CREATE INDEX scan_jobs_status_idx ON scan_jobs (status, created_at);

CREATE TYPE scan_decision AS ENUM ('pending', 'approved', 'rejected');

CREATE TABLE scan_results (
    id                  BIGSERIAL PRIMARY KEY,
    scan_job_id         BIGINT NOT NULL REFERENCES scan_jobs(id) ON DELETE CASCADE,
    ip                  INET NOT NULL,
    sys_object_id       TEXT,
    sys_descr           TEXT,
    sys_name            TEXT,
    vendor              TEXT,
    profile             TEXT,
    already_monitored   BOOLEAN NOT NULL DEFAULT FALSE,
    decision            scan_decision NOT NULL DEFAULT 'pending',
    device_id           BIGINT REFERENCES devices(id) ON DELETE SET NULL,
    found_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (scan_job_id, ip)
);

-- A device is identified by its management address; scans must not create duplicates.
CREATE UNIQUE INDEX devices_mgmt_ip_key ON devices (mgmt_ip);
