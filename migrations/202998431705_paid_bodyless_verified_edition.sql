-- FND-00 / #1103: additive SQL guards for paid S3-only editions.
-- Old SQLx migrations are immutable. Existing paid PG-body editions keep working.
-- This migration permits verified bodyless paid attachments; it does NOT by
-- itself switch the authoring service away from PostgreSQL plaintext.
--
-- G's verified object receipt proves exact object bytes through the provider
-- adapter. This SQL contract checks that the receipt is for the immutable
-- B edition, the version-bound AAD, and the attached historical object.

CREATE OR REPLACE FUNCTION reject_chapter_version_update()
RETURNS trigger LANGUAGE plpgsql AS $paid_bodyless_attach$
DECLARE
    original_sha BYTEA;
BEGIN
    IF to_jsonb(NEW) - 'storage_object_id'
        IS DISTINCT FROM to_jsonb(OLD) - 'storage_object_id'
        OR OLD.storage_object_id IS NOT NULL
        OR NEW.storage_object_id IS NULL THEN
        RAISE EXCEPTION 'chapter version must be immutable'
            USING ERRCODE='23514';
    END IF;

    IF OLD.body_markdown IS NULL THEN
        -- S3-only versions reserve the immutable B digest before G uploads.
        -- Never accept a bodyless version with an unknown/hashless identity.
        original_sha := OLD.content_sha256;
    ELSE
        -- Backwards compatibility for existing paid chapter versions.
        original_sha := sha256(convert_to(
            replace(replace(OLD.body_markdown, E'\r\n', E'\n'), E'\r', E'\n'),
            'UTF8'
        ));
        IF OLD.content_sha256 IS NOT NULL
            AND OLD.content_sha256 IS DISTINCT FROM original_sha THEN
            RAISE EXCEPTION 'paid body digest conflicts with edition digest'
                USING ERRCODE='23514';
        END IF;
    END IF;

    IF original_sha IS NULL OR NOT EXISTS (
        SELECT 1 FROM storage_objects s
        JOIN chapters c ON c.id=OLD.chapter_id AND c.access_level='paid'
        WHERE s.id=NEW.storage_object_id
            AND s.object_kind='chapter'
            AND s.state='attached'
            AND s.plaintext_sha256=original_sha
            AND s.object_aad=convert_to(
                'tor.encrypted.chapter.v1' || E'\n' ||
                OLD.chapter_id::text || E'\n' ||
                OLD.id::text || E'\n' ||
                s.id::text || E'\n' ||
                encode(original_sha,'hex') || E'\n',
                'UTF8'
            )
    ) THEN
        RAISE EXCEPTION 'unverified or mismatched paid chapter object'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$paid_bodyless_attach$;

-- The parser manifest is immutable and must match both the B edition and
-- the G-attached receipt. S3-only paid versions use the immutable digest;
-- pre-existing PG-body versions additionally verify the actual PG text.
CREATE OR REPLACE FUNCTION require_verified_reader_manifest()
RETURNS trigger LANGUAGE plpgsql AS $paid_bodyless_manifest$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM chapter_versions v
        JOIN chapters c ON c.id=v.chapter_id AND c.access_level='paid'
        JOIN storage_objects o ON o.id=v.storage_object_id
        WHERE v.chapter_id=NEW.chapter_id
            AND v.id=NEW.version_id
            AND v.storage_object_id=NEW.storage_object_id
            AND o.object_kind='chapter'
            AND o.codec='gzip'
            AND o.state='attached'
            AND o.plaintext_sha256=NEW.content_sha256
            AND (v.content_sha256 IS NULL OR
                 v.content_sha256=NEW.content_sha256)
            AND (
                (v.body_markdown IS NULL
                 AND v.content_sha256=NEW.content_sha256)
                OR
                (v.body_markdown IS NOT NULL
                 AND sha256(convert_to(
                     replace(replace(v.body_markdown,E'\r\n',E'\n'),E'\r',E'\n'),
                     'UTF8'
                 ))=NEW.content_sha256)
            )
    ) THEN
        RAISE EXCEPTION
            'reader manifest must match verified immutable encrypted version'
            USING ERRCODE='23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM unnest(NEW.block_utf16_lengths) AS b(len)
        WHERE b.len < 0 OR b.len > 100000
    ) THEN
        RAISE EXCEPTION 'invalid reader manifest block length'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$paid_bodyless_manifest$;

-- An S3-only paid edition must never become the reader's CURRENT pointer
-- before its exact encrypted object and immutable parser manifest exist.
-- For old editions with PG bodies, preserve the existing migration behavior.
CREATE FUNCTION guard_paid_bodyless_publication()
RETURNS trigger LANGUAGE plpgsql AS $paid_bodyless_publication$
BEGIN
    IF EXISTS (
        SELECT 1 FROM chapter_versions v
        JOIN chapters c ON c.id=v.chapter_id
        WHERE v.chapter_id=NEW.chapter_id
            AND v.id=NEW.version_id
            AND c.access_level='paid'
            AND v.body_markdown IS NULL
            AND NOT EXISTS (
                SELECT 1 FROM chapter_reader_manifests m
                JOIN storage_objects o
                    ON o.id=m.storage_object_id
                    AND o.state='attached'
                    AND o.object_kind='chapter'
                WHERE m.chapter_id=v.chapter_id
                    AND m.version_id=v.id
                    AND m.storage_object_id=v.storage_object_id
                    AND m.content_sha256=v.content_sha256
                    AND o.plaintext_sha256=v.content_sha256
            )
    ) THEN
        RAISE EXCEPTION
            'paid S3-only publication requires attached ciphertext and manifest'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$paid_bodyless_publication$;

CREATE TRIGGER paid_bodyless_publication_guard
    BEFORE INSERT OR UPDATE ON chapter_publications
    FOR EACH ROW EXECUTE FUNCTION guard_paid_bodyless_publication();

COMMENT ON FUNCTION guard_paid_bodyless_publication() IS
    'FND-00: bodyless paid version requires verified attached S3 and immutable reader manifest; not an entitlement';
