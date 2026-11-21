CREATE TABLE raw_samples (
    device_id    BIGINT NOT NULL,
    metric_id    INTEGER NOT NULL,
    interface_id BIGINT NOT NULL DEFAULT 0,
    ts           TIMESTAMPTZ NOT NULL,
    value        DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (device_id, metric_id, interface_id, ts)
) PARTITION BY RANGE (ts);

CREATE INDEX raw_samples_device_metric_ts ON raw_samples (device_id, metric_id, ts);