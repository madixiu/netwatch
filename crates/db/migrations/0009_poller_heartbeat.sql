CREATE TABLE poller_heartbeat (
    name    TEXT PRIMARY KEY,        -- e.g. 'default'
    ts      TIMESTAMPTZ NOT NULL DEFAULT now(),
    version TEXT
);