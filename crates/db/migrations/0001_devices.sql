CREATE TABLE devices (
    id                BIGSERIAL PRIMARY KEY,
    name              TEXT NOT NULL UNIQUE,
    mgmt_ip           INET NOT NULL,
    vendor            TEXT NOT NULL,
    model             TEXT,
    profile           TEXT NOT NULL,
    snmp_version      SMALLINT NOT NULL CHECK (snmp_version IN (2, 3)),
    poll_interval_secs INTEGER NOT NULL DEFAULT 60,
    enabled           BOOLEAN NOT NULL DEFAULT TRUE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);