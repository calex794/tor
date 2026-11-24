-- S3-ADMIN / 2026-11-23. A saved verified connection is NOT a production cutover.
-- No object, plaintext chapter or payment key is stored in this table.
CREATE TABLE admin_s3_profiles (
    id SMALLINT PRIMARY KEY CHECK (id=1),
    provider TEXT NOT NULL DEFAULT 'yandex' CHECK (provider='yandex'),
    endpoint TEXT NOT NULL DEFAULT 'https://storage.yandexcloud.net'
        CHECK (endpoint='https://storage.yandexcloud.net'),
    region TEXT NOT NULL DEFAULT 'ru-central1' CHECK (region='ru-central1'),
    bucket TEXT NOT NULL,
    ciphertext BYTEA NOT NULL CHECK (octet_length(ciphertext)>=17),
    nonce BYTEA NOT NULL CHECK (octet_length(nonce)=12),
    encryption_version SMALLINT NOT NULL DEFAULT 1 CHECK (encryption_version=1),
    verified_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    active BOOLEAN NOT NULL DEFAULT FALSE CHECK (active=FALSE)
);
COMMENT ON TABLE admin_s3_profiles IS
    'Admin-verified encrypted S3 profile, not automatic publish activation. TOR_S3_ADMIN_MASTER_KEY must be backed up separately. Never log or return ciphertext/key.';
CREATE TABLE admin_s3_profile_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    performed_by_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    action TEXT NOT NULL CHECK (action='save_verified_connection'),
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
COMMENT ON TABLE admin_s3_profile_audit IS
    'No credentials, bucket, headers, endpoint or reader identities recorded. Admin connection saves only.';
