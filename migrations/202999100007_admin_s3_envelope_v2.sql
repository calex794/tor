-- Additive S3-ADMIN envelope upgrade. V1 rows remain readable as metadata
-- only and must be re-entered; never silently treat V1 as bucket-bound V2.
-- Keep the original default for old clients. New writes explicitly select 2.
ALTER TABLE admin_s3_profiles
    DROP CONSTRAINT admin_s3_profiles_encryption_version_check,
    ADD CONSTRAINT admin_s3_profiles_encryption_version_check
        CHECK (encryption_version IN (1, 2));
COMMENT ON COLUMN admin_s3_profiles.encryption_version IS
    'V1 legacy metadata only; V2 binds provider, purpose and bucket to AES-GCM. Neither activates S3.';
