-- B/G S065: exact immutable paid edition -> private encrypted image object.
-- 006 belongs to the parallel E-01 comments PR. No prior DDL is rewritten.
-- An asset is NOT an entitlement. C still must resolve D before any G grant.
CREATE TABLE chapter_version_assets (
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    storage_object_id UUID NOT NULL UNIQUE
        REFERENCES storage_objects(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('image/png','image/jpeg','image/webp')),
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 1 AND 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chapter_id, version_id, ordinal),
    FOREIGN KEY (chapter_id, version_id)
        REFERENCES chapter_versions(chapter_id, id) ON DELETE RESTRICT
);

-- Membership can only be committed for an actual verified, attached object.
-- Asset AAD binds chapter, immutable edition, object UUID and content hash;
-- swapping a V2 image into V1, a public object or a chapter body fails closed.
CREATE FUNCTION require_verified_paid_chapter_asset()
RETURNS trigger LANGUAGE plpgsql AS $paid_asset_verified$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM chapter_versions AS v
        JOIN chapters AS c ON c.id=v.chapter_id
        JOIN storage_objects AS chapter_o
          ON chapter_o.id=v.storage_object_id AND chapter_o.state='attached'
          AND chapter_o.object_kind='chapter'
        JOIN storage_objects AS image_o
          ON image_o.id=NEW.storage_object_id
        WHERE v.chapter_id=NEW.chapter_id AND v.id=NEW.version_id
          AND c.access_level='paid' AND NOT c.is_deleted AND NOT c.admin_hold
          AND image_o.state='attached'
          AND image_o.object_kind='chapter_asset'
          AND image_o.codec='none'
          AND image_o.object_aad=convert_to(
              'tor.encrypted.asset.v1' || E'\n' ||
              NEW.chapter_id::text || E'\n' ||
              NEW.version_id::text || E'\n' ||
              image_o.id::text || E'\n' ||
              encode(image_o.plaintext_sha256,'hex') || E'\n',
              'UTF8'
          )
    ) THEN
        RAISE EXCEPTION 'asset must belong to verified paid chapter edition'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$paid_asset_verified$;

CREATE TRIGGER paid_chapter_asset_verified
    BEFORE INSERT ON chapter_version_assets
    FOR EACH ROW EXECUTE FUNCTION require_verified_paid_chapter_asset();

CREATE FUNCTION immutable_paid_chapter_asset()
RETURNS trigger LANGUAGE plpgsql AS $paid_asset_immutable$
BEGIN
    RAISE EXCEPTION 'paid chapter asset association is immutable'
        USING ERRCODE='23514';
END;
$paid_asset_immutable$;

CREATE TRIGGER paid_chapter_asset_immutable
    BEFORE UPDATE OR DELETE ON chapter_version_assets
    FOR EACH ROW EXECUTE FUNCTION immutable_paid_chapter_asset();

-- An operator may not remove an edition's image and reuse the ciphertext ID.
-- Attached objects are already protected by the G trigger plus this FK.
CREATE INDEX chapter_version_assets_edition_idx
    ON chapter_version_assets (chapter_id, version_id, ordinal);
