CREATE TABLE device_probe_results (
    id BIGSERIAL PRIMARY KEY,
    device_id BIGINT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    protocol TEXT NOT NULL CHECK (protocol IN ('icmp', 'snmp')),
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    status TEXT NOT NULL CHECK (status IN ('ok', 'failed', 'error')),
    latency_ms DOUBLE PRECISION CHECK (latency_ms IS NULL OR latency_ms >= 0),
    detail TEXT,
    CHECK (status = 'ok' OR latency_ms IS NULL)
);

CREATE INDEX device_probe_results_latest
    ON device_probe_results (device_id, protocol, observed_at DESC, id DESC);
CREATE INDEX device_probe_results_retention ON device_probe_results (observed_at);
