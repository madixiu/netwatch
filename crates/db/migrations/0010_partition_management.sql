-- Create one daily partition for `day` if it does not exist.
CREATE OR REPLACE FUNCTION create_raw_partition(day DATE) RETURNS VOID AS $$
DECLARE
    part_name TEXT := format('raw_samples_%s', to_char(day, 'YYYYMMDD'));
    start_ts  TIMESTAMPTZ := day::TIMESTAMPTZ;
    end_ts    TIMESTAMPTZ := (day + INTERVAL '1 day')::TIMESTAMPTZ;
BEGIN
    EXECUTE format(
        'CREATE TABLE IF NOT EXISTS %I PARTITION OF raw_samples FOR VALUES FROM (%L) TO (%L)',
        part_name, start_ts, end_ts
    );
END;
$$ LANGUAGE plpgsql;

-- Create partitions for today through today+days_ahead.
CREATE OR REPLACE FUNCTION ensure_raw_partitions(days_ahead INT DEFAULT 7) RETURNS INT AS $$
DECLARE
    i INT;
    created INT := 0;
BEGIN
    FOR i IN 0..days_ahead LOOP
        PERFORM create_raw_partition(CURRENT_DATE + i);
        created := created + 1;
    END LOOP;
    RETURN created;
END;
$$ LANGUAGE plpgsql;

-- Drop raw partitions whose date is older than older_than_days.
-- We own the naming convention (raw_samples_YYYYMMDD), so parsing is safe.
CREATE OR REPLACE FUNCTION drop_old_raw_partitions(older_than_days INT) RETURNS INT AS $$
DECLARE
    cutoff DATE := CURRENT_DATE - older_than_days;
    part   RECORD;
    dropped INT := 0;
BEGIN
    FOR part IN
        SELECT tablename FROM pg_tables
        WHERE schemaname = 'public' AND tablename ~ '^raw_samples_[0-9]{8}$'
    LOOP
        IF substring(part.tablename FROM 13)::DATE < cutoff THEN
            EXECUTE format('DROP TABLE IF EXISTS %I', part.tablename);
            dropped := dropped + 1;
        END IF;
    END LOOP;
    RETURN dropped;
END;
$$ LANGUAGE plpgsql;

-- Bootstrap the next 7 days immediately so phase 1 does not race midnight.
SELECT ensure_raw_partitions(7);