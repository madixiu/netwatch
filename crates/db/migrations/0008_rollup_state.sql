CREATE TABLE rollup_state (
    granularity      TEXT PRIMARY KEY,  -- '1m' | '1h' | '1d'
    last_rolled_up_at TIMESTAMPTZ
);
INSERT INTO rollup_state (granularity) VALUES ('1m'), ('1h'), ('1d');