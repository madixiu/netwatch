ALTER TABLE rollup_1m
    ADD COLUMN sum_value DOUBLE PRECISION,
    ADD COLUMN sum_delta DOUBLE PRECISION,
    ADD COLUMN sum_seconds DOUBLE PRECISION,
    ADD COLUMN reset_count BIGINT NOT NULL DEFAULT 0;
ALTER TABLE rollup_1h
    ADD COLUMN sum_value DOUBLE PRECISION,
    ADD COLUMN sum_delta DOUBLE PRECISION,
    ADD COLUMN sum_seconds DOUBLE PRECISION,
    ADD COLUMN reset_count BIGINT NOT NULL DEFAULT 0;
ALTER TABLE rollup_1d
    ADD COLUMN sum_value DOUBLE PRECISION,
    ADD COLUMN sum_delta DOUBLE PRECISION,
    ADD COLUMN sum_seconds DOUBLE PRECISION,
    ADD COLUMN reset_count BIGINT NOT NULL DEFAULT 0;

UPDATE rollup_1m SET sum_value = avg * count WHERE sum_value IS NULL;
UPDATE rollup_1h SET sum_value = avg * count WHERE sum_value IS NULL;
UPDATE rollup_1d SET sum_value = avg * count WHERE sum_value IS NULL;
