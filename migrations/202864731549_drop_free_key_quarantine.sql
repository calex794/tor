-- DROP-07h: publication and quarantine compete for the same permanent key.
CREATE TABLE free_storage_key_fences (
    bucket TEXT NOT NULL CHECK (
        bucket ~ '^[a-z][a-z0-9-]{1,61}[a-z0-9]$' AND bucket NOT LIKE 'public%'
    ),
    object_key TEXT NOT NULL CHECK (
        char_length(object_key) <= 260
        AND object_key ~ '^private/(free-chapters/[1-9][0-9]*/[1-9][0-9]*[.]md|free-packs/[A-Za-z0-9_./-]+[.](zst|manifest[.]json))$'
        AND position('//' in object_key) = 0
        AND object_key !~ '(^|/)[.][.]?(/|$)'
    ),
    purpose TEXT NOT NULL CHECK (purpose IN ('publisher', 'quarantine')),
    object_version TEXT CHECK (
        octet_length(object_version) BETWEEN 1 AND 1024
        AND object_version <> 'null' AND object_version !~ '[[:cntrl:]]'
    ),
    etag TEXT CHECK (octet_length(etag) BETWEEN 3 AND 256 AND etag !~ '[[:cntrl:]]'),
    sha256 BYTEA CHECK (octet_length(sha256) = 32 AND sha256 <> decode(repeat('00',32),'hex')),
    size_bytes BIGINT CHECK (size_bytes BETWEEN 1 AND 16777216),
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (bucket, object_key),
    CHECK (
        (purpose='publisher' AND object_version IS NULL AND etag IS NULL
         AND sha256 IS NULL AND size_bytes IS NULL) OR
        (purpose='quarantine' AND object_version IS NOT NULL AND etag IS NOT NULL
         AND sha256 IS NOT NULL AND size_bytes IS NOT NULL)
    )
);
CREATE TRIGGER free_storage_key_fences_immutable
    BEFORE UPDATE OR DELETE ON free_storage_key_fences FOR EACH ROW
    EXECUTE FUNCTION reject_free_chapter_storage_change();

CREATE FUNCTION reserve_free_storage_publication(p_bucket TEXT, p_key TEXT)
RETURNS VOID LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO free_storage_key_fences(bucket,object_key,purpose)
    VALUES(p_bucket,p_key,'publisher') ON CONFLICT(bucket,object_key) DO NOTHING;
    IF NOT EXISTS (
        SELECT 1 FROM free_storage_key_fences
         WHERE bucket=p_bucket AND object_key=p_key AND purpose='publisher'
    ) THEN
        RAISE EXCEPTION 'DROP: publication blocked by quarantined storage identity'
            USING ERRCODE = '23514';
    END IF;
END;
$$;

-- Receipt creation cannot bypass quarantine, even from an older publisher.
CREATE FUNCTION fence_free_storage_receipt()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    locator_bucket TEXT;
    locator_key TEXT;
    sidecar_key TEXT;
BEGIN
    IF TG_TABLE_NAME = 'free_pack_job_specs' THEN
        locator_bucket := NEW.spec_text::jsonb #>> '{target,bucket}';
        locator_key := NEW.spec_text::jsonb #>> '{target,pack_key}';
        sidecar_key := NEW.spec_text::jsonb #>> '{target,manifest_key}';
    ELSE
        locator_bucket := NEW.bucket;
        locator_key := NEW.object_key;
        IF TG_TABLE_NAME = 'free_storage_packs' THEN
            sidecar_key := NEW.manifest_key;
        ELSIF TG_TABLE_NAME = 'free_pack_direct_import_stages' THEN
            sidecar_key := regexp_replace(locator_key,'[.]zst$','.manifest.json');
        END IF;
    END IF;
    -- Both PACK components use the same lock order in SQL and Rust.
    IF sidecar_key IS NOT NULL THEN
        PERFORM reserve_free_storage_publication(locator_bucket,sidecar_key);
    END IF;
    PERFORM reserve_free_storage_publication(locator_bucket,locator_key);
    RETURN NEW;
END;
$$;

CREATE TRIGGER free_storage_receipt_single_fence
    BEFORE INSERT ON free_chapter_storage_objects FOR EACH ROW
    EXECUTE FUNCTION fence_free_storage_receipt();
CREATE TRIGGER free_storage_receipt_pack_fence
    BEFORE INSERT ON free_storage_packs FOR EACH ROW
    EXECUTE FUNCTION fence_free_storage_receipt();
CREATE TRIGGER free_storage_receipt_stage_fence
    BEFORE INSERT ON free_pack_direct_import_stages FOR EACH ROW
    EXECUTE FUNCTION fence_free_storage_receipt();
CREATE TRIGGER free_storage_receipt_job_fence
    BEFORE INSERT ON free_pack_job_specs FOR EACH ROW
    EXECUTE FUNCTION fence_free_storage_receipt();

CREATE TABLE free_storage_quarantine_rechecks (
    bucket TEXT NOT NULL,
    object_key TEXT NOT NULL,
    backup_bucket TEXT NOT NULL CHECK (
        backup_bucket ~ '^[a-z][a-z0-9-]{1,61}[a-z0-9]$'
        AND backup_bucket NOT LIKE 'public%'
    ),
    backup_key TEXT NOT NULL CHECK (
        octet_length(backup_key) BETWEEN 1 AND 1024 AND backup_key !~ '[[:cntrl:]]'
    ),
    backup_version TEXT NOT NULL CHECK (
        octet_length(backup_version) BETWEEN 1 AND 1024 AND backup_version <> 'null'
        AND backup_version !~ '[[:cntrl:]]'
    ),
    confirmed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(bucket,object_key),
    FOREIGN KEY(bucket,object_key) REFERENCES free_storage_key_fences(bucket,object_key),
    CHECK (backup_bucket <> bucket)
);
CREATE TRIGGER free_storage_quarantine_rechecks_immutable
    BEFORE UPDATE OR DELETE ON free_storage_quarantine_rechecks FOR EACH ROW
    EXECUTE FUNCTION reject_free_chapter_storage_change();
