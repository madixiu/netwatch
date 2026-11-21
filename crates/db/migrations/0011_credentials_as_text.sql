-- Upgrade path for installs that ran the old BYTEA-based 0002_credentials.sql.
-- Fresh installs already have TEXT columns; this migration is a no-op there.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'credentials'
          AND column_name = 'snmp_v2c_community_enc'
          AND data_type = 'bytea'
    ) THEN
        ALTER TABLE credentials
            ALTER COLUMN snmp_v2c_community_enc  TYPE TEXT USING convert_from(snmp_v2c_community_enc,  'UTF8'),
            ALTER COLUMN snmp_v3_auth_secret_enc TYPE TEXT USING convert_from(snmp_v3_auth_secret_enc, 'UTF8'),
            ALTER COLUMN snmp_v3_priv_secret_enc TYPE TEXT USING convert_from(snmp_v3_priv_secret_enc, 'UTF8');
    END IF;
END $$;