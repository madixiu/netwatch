CREATE TABLE hardware_snapshot (
    device_id BIGINT PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    status TEXT NOT NULL CHECK (status IN ('ok', 'partial', 'unavailable', 'error')),
    detail TEXT
);

CREATE TABLE hardware_components (
    device_id BIGINT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    entity_index INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('temperature', 'fan', 'power')),
    name TEXT NOT NULL,
    status TEXT,
    value DOUBLE PRECISION,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (device_id, entity_index)
);
