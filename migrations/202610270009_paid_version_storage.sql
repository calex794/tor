-- I/B S3-09/11: one-time immutable ciphertext attachment to a B-owned
-- chapter edition. Legacy free/paid versions remain readable as before.
-- Only the verified G object and the exact historical version may link.
ALTER TABLE chapter_versions
    ADD COLUMN storage_object_id UUID UNIQUE
        REFERENCES storage_objects(id) ON DELETE RESTRICT;

-- Original chapter-version trigger rejected every UPDATE. Replace it with an
-- exactly-once storage attachment exception, keeping ALL body/metadata columns
-- immutable, including columns added in future migrations.
CREATE OR REPLACE FUNCTION reject_chapter_version_update()
RETURNS trigger LANGUAGE plpgsql AS $chapter_version_storage_attach$
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

    -- Rust normalizes CRLF/CR to LF before encryption; verify that the stored
    -- G plaintext digest matches the actual B-owned historical Markdown.
    original_sha := sha256(convert_to(
        replace(replace(OLD.body_markdown, E'\r\n', E'\n'), E'\r', E'\n'),
        'UTF8'
    ));
    IF NOT EXISTS (
        SELECT 1 FROM storage_objects s
        JOIN chapters c ON c.id=OLD.chapter_id AND c.access_level='paid'
        WHERE s.id=NEW.storage_object_id AND s.object_kind='chapter'
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
$chapter_version_storage_attach$;

-- Durable B publication fact, inserted in the SAME transaction as the
-- pointer move and object attach, and not an indication of outbox delivery.
CREATE TABLE chapter_encrypted_release_events (
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    storage_object_id UUID NOT NULL REFERENCES storage_objects(id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chapter_id,version_id),
    FOREIGN KEY (chapter_id,version_id)
        REFERENCES chapter_versions(chapter_id,id) ON DELETE RESTRICT
);

CREATE FUNCTION enforce_encrypted_release_fact()
RETURNS trigger LANGUAGE plpgsql AS $encrypted_release_fact$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM chapter_versions v
        JOIN storage_objects o ON o.id=v.storage_object_id AND o.state='attached'
        WHERE v.chapter_id=NEW.chapter_id AND v.id=NEW.version_id
          AND v.storage_object_id=NEW.storage_object_id
    ) THEN
        RAISE EXCEPTION 'release fact must reference attached historical version'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$encrypted_release_fact$;

CREATE TRIGGER encrypted_release_fact_verified
    BEFORE INSERT ON chapter_encrypted_release_events
    FOR EACH ROW EXECUTE FUNCTION enforce_encrypted_release_fact();

CREATE FUNCTION immutable_encrypted_release_fact()
RETURNS trigger LANGUAGE plpgsql AS $immutable_release_fact$
BEGIN
    RAISE EXCEPTION 'encrypted release history is immutable'
        USING ERRCODE='23514';
END;
$immutable_release_fact$;

CREATE TRIGGER encrypted_release_fact_immutable
    BEFORE UPDATE OR DELETE ON chapter_encrypted_release_events
    FOR EACH ROW EXECUTE FUNCTION immutable_encrypted_release_fact();
