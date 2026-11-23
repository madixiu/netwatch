CREATE TABLE mac_table (
    device_id    BIGINT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    interface_id BIGINT NOT NULL REFERENCES interfaces(id) ON DELETE CASCADE,
    mac          TEXT NOT NULL,
    last_seen    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (device_id, mac)
);
CREATE INDEX mac_table_interface_idx ON mac_table (interface_id);
