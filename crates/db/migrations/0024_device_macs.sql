CREATE TABLE device_macs (
    device_id BIGINT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    mac       TEXT NOT NULL,
    PRIMARY KEY (device_id, mac)
);
CREATE INDEX device_macs_mac_idx ON device_macs (mac);
