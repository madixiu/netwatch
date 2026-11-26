-- Quiet polling: 5-minute default. Existing devices below 5 minutes move to 5 minutes.
ALTER TABLE devices ALTER COLUMN poll_interval_secs SET DEFAULT 300;
UPDATE devices SET poll_interval_secs = 300 WHERE poll_interval_secs < 300;
