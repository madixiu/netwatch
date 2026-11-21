CREATE TABLE rollup_1m (
    device_id    BIGINT NOT NULL,
    metric_id    INTEGER NOT NULL,
    interface_id BIGINT NOT NULL DEFAULT 0,
    bucket       TIMESTAMPTZ NOT NULL,
    min          DOUBLE PRECISION NOT NULL,
    max          DOUBLE PRECISION NOT NULL,
    avg          DOUBLE PRECISION NOT NULL,
    last         DOUBLE PRECISION NOT NULL,
    count        BIGINT NOT NULL,
    PRIMARY KEY (device_id, metric_id, interface_id, bucket)
);
CREATE TABLE rollup_1h (LIKE rollup_1m INCLUDING ALL);
CREATE TABLE rollup_1d (LIKE rollup_1m INCLUDING ALL);