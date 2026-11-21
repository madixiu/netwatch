-- Historical interface_id values may be raw ifIndex values rather than row IDs.
ALTER TABLE raw_samples ADD COLUMN mapping_verified BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE rollup_1m ADD COLUMN mapping_verified BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE rollup_1h ADD COLUMN mapping_verified BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE rollup_1d ADD COLUMN mapping_verified BOOLEAN NOT NULL DEFAULT false;
