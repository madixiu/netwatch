-- SNMP credentials per device. Encrypted fields are base64 ciphertext from
-- `netwatch_crypto::seal` (format: `v1:<base64>`). Plaintext never touches
-- this table. If you have already applied the *old* 0002 (BYTEA columns),
-- run `0011_credentials_as_text.sql` instead of re-running this file.
CREATE TABLE credentials (
    device_id                 BIGINT PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
    snmp_v2c_community_enc    TEXT,
    snmp_v3_username          TEXT,
    snmp_v3_auth_proto        TEXT,
    snmp_v3_auth_secret_enc   TEXT,
    snmp_v3_priv_proto        TEXT,
    snmp_v3_priv_secret_enc   TEXT,
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now()
);