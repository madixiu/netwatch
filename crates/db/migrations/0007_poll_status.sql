CREATE TABLE poll_status (
    device_id   BIGINT NOT NULL,
    ts          TIMESTAMPTZ NOT NULL DEFAULT now(),
    ok          BOOLEAN NOT NULL,
    duration_ms INTEGER,
    error       TEXT,
    PRIMARY KEY (device_id, ts)
);
CREATE INDEX poll_status_device_ts_desc ON poll_status (device_id, ts DESC);