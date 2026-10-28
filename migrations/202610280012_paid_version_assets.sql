-- I/B S065: version-exact private image membership. No public media path.
-- A paid image must be attached only while its B edition is still a draft.
-- Published V1's membership is immutable even after a later V2 is released.
CREATE TABLE chapter_version_assets (
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    storage_object_id UUID NOT NULL UNIQUE
        REFERENCES storage_objects(id) ON DELETE RESTRICT,
    ordinal SMALLINT NOT NULL CHECK (ordinal BETWEEN 1 AND 64),
    media_type TEXT NOT NULL
        CHECK (media_type IN ('image/png','image/jpeg','image/webp')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chapter_id,version_id,ordinal),
    FOREIGN KEY (chapter_id,version_id)
        REFERENCES chapter_versions(chapter_id,id) ON DELETE RESTRICT
);

CREATE FUNCTION verify_paid_version_asset()
RETURNS trigger LANGUAGE plpgsql AS $paid_asset_verified$
BEGIN
    -- Never change the historical contents of a purchased/published edition.
    IF EXISTS (
        SELECT 1 FROM chapter_publications
        WHERE chapter_id=NEW.chapter_id AND version_id=NEW.version_id
    ) OR EXISTS (
        SELECT 1 FROM chapter_encrypted_release_events
        WHERE chapter_id=NEW.chapter_id AND version_id=NEW.version_id
    ) THEN
        RAISE EXCEPTION 'cannot add an image after edition release'
            USING ERRCODE='23514';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM chapter_versions v
        JOIN chapters c ON c.id=v.chapter_id
        JOIN storage_objects o ON o.id=NEW.storage_object_id
        WHERE v.chapter_id=NEW.chapter_id AND v.id=NEW.version_id
          AND v.storage_object_id IS NULL
          AND c.access_level='paid' AND c.price_amount>0
          AND o.state='attached' AND o.object_kind='chapter_asset'
          AND o.codec='none'
          AND o.object_key='private/chapter-assets/' || o.id::text || '.enc'
          AND o.object_aad=convert_to(
              'tor.encrypted.asset.v1' || E'\n' ||
              NEW.chapter_id::text || E'\n' ||
              NEW.version_id::text || E'\n' ||
              o.id::text || E'\n' ||
              encode(o.plaintext_sha256,'hex') || E'\n', 'UTF8'
          )
    ) THEN
        RAISE EXCEPTION 'image is not a verified object of this exact paid draft'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$paid_asset_verified$;

CREATE TRIGGER chapter_version_asset_verified
    BEFORE INSERT ON chapter_version_assets
    FOR EACH ROW EXECUTE FUNCTION verify_paid_version_asset();

CREATE FUNCTION reject_chapter_version_asset_change()
RETURNS trigger LANGUAGE plpgsql AS $paid_asset_immutable$
BEGIN
    RAISE EXCEPTION 'published edition image membership is immutable'
        USING ERRCODE='23514';
END;
$paid_asset_immutable$;

CREATE TRIGGER chapter_version_asset_immutable
    BEFORE UPDATE OR DELETE ON chapter_version_assets
    FOR EACH ROW EXECUTE FUNCTION reject_chapter_version_asset_change();
