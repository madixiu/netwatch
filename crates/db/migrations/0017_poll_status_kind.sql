ALTER TABLE poll_status ADD COLUMN status TEXT;
UPDATE poll_status SET status = CASE WHEN ok THEN 'ok' ELSE 'error' END;
ALTER TABLE poll_status
    ALTER COLUMN status SET NOT NULL,
    ADD CONSTRAINT poll_status_kind CHECK (status IN ('ok', 'partial', 'error'));
