CREATE TABLE metrics_catalog (
    id   SERIAL PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    unit TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('counter', 'gauge'))
);