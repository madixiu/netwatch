ALTER TABLE raw_samples
    ADD COLUMN delta DOUBLE PRECISION,
    ADD COLUMN dt_seconds DOUBLE PRECISION,
    ADD COLUMN counter_reset BOOLEAN NOT NULL DEFAULT false,
    ADD CONSTRAINT raw_samples_counter_delta_pair CHECK (
        (delta IS NULL AND dt_seconds IS NULL)
        OR (delta IS NOT NULL AND dt_seconds IS NOT NULL
            AND delta >= 0 AND dt_seconds > 0 AND NOT counter_reset)
    );
