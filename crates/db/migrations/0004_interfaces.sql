CREATE TABLE interfaces (
    id          BIGSERIAL PRIMARY KEY,
    device_id   BIGINT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    if_index    INTEGER NOT NULL,
    name        TEXT NOT NULL,
    speed_bps   BIGINT,
    removed_at  TIMESTAMPTZ,
    UNIQUE (device_id, if_index)
);