-- I/G S3-08: immutable private ciphertext registry. No paid reader API.
-- State is a G-owned upload receipt, NOT proof of a D entitlement or a
-- B publication. B version association is introduced separately, with its
-- owner-controlled publication transaction. No raw content DEK is stored.
CREATE TABLE storage_objects (
    id UUID PRIMARY KEY,
    object_kind TEXT NOT NULL CHECK (object_kind IN ('chapter','chapter_asset')),
    bucket TEXT NOT NULL CHECK (
        bucket ~ '^[a-z][a-z0-9-]{1,61}[a-z0-9]$'
        AND bucket NOT LIKE 'public%'
    ),
    object_key TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending','ready','attached','deleting')),
    size_bytes BIGINT NOT NULL CHECK (size_bytes BETWEEN 17 AND 16777216),
    sha256_ciphertext BYTEA NOT NULL CHECK (
        octet_length(sha256_ciphertext)=32
        AND sha256_ciphertext <> decode(repeat('00',32),'hex')
    ),
    plaintext_sha256 BYTEA NOT NULL CHECK (octet_length(plaintext_sha256)=32),
    encrypted_dek BYTEA NOT NULL CHECK (octet_length(encrypted_dek)=48),
    wrap_nonce BYTEA NOT NULL CHECK (octet_length(wrap_nonce)=12),
    kek_id TEXT NOT NULL CHECK (kek_id='content-kek-v1'),
    object_nonce BYTEA NOT NULL CHECK (octet_length(object_nonce)=12),
    object_aad BYTEA NOT NULL CHECK (octet_length(object_aad) BETWEEN 32 AND 512),
    codec TEXT NOT NULL CHECK (
        (object_kind='chapter' AND codec='gzip')
        OR (object_kind='chapter_asset' AND codec='none')
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    verified_at TIMESTAMPTZ,
    attached_at TIMESTAMPTZ,
    delete_after TIMESTAMPTZ,
    CONSTRAINT storage_key_matches_object CHECK (
        (object_kind='chapter'
         AND object_key='private/chapter-versions/' || id::text || '.enc')
        OR
        (object_kind='chapter_asset'
         AND object_key='private/chapter-assets/' || id::text || '.enc')
    ),
    CONSTRAINT storage_state_dates CHECK (
        (state='pending' AND verified_at IS NULL AND attached_at IS NULL)
        OR
        (state='ready' AND verified_at IS NOT NULL AND attached_at IS NULL)
        OR
        (state='attached' AND verified_at IS NOT NULL
            AND attached_at IS NOT NULL AND delete_after IS NULL)
        OR
        (state='deleting' AND attached_at IS NULL)
    )
);
CREATE INDEX storage_objects_unattached_gc_idx
    ON storage_objects(delete_after,id)
    WHERE state IN ('pending','ready','deleting') AND delete_after IS NOT NULL;

-- A purchased V1 must never be overwritten by a later revision, even by a
-- flawed retry or operator SQL. Only lifecycle state and timestamps change.
CREATE FUNCTION guard_storage_object_immutability()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(
        NEW.id,NEW.object_kind,NEW.bucket,NEW.object_key,NEW.size_bytes,
        NEW.sha256_ciphertext,NEW.plaintext_sha256,NEW.encrypted_dek,
        NEW.wrap_nonce,NEW.kek_id,NEW.object_nonce,NEW.object_aad,NEW.codec
    ) IS DISTINCT FROM ROW(
        OLD.id,OLD.object_kind,OLD.bucket,OLD.object_key,OLD.size_bytes,
        OLD.sha256_ciphertext,OLD.plaintext_sha256,OLD.encrypted_dek,
        OLD.wrap_nonce,OLD.kek_id,OLD.object_nonce,OLD.object_aad,OLD.codec
    ) THEN
        RAISE EXCEPTION 'immutable private storage object'
            USING ERRCODE='23514';
    END IF;
    IF NEW.state<>OLD.state AND NOT (
        (OLD.state='pending' AND NEW.state IN ('ready','deleting'))
        OR (OLD.state='ready' AND NEW.state IN ('attached','deleting'))
    ) THEN
        RAISE EXCEPTION 'invalid private storage transition'
            USING ERRCODE='23514';
    END IF;
    IF OLD.verified_at IS NOT NULL AND NEW.verified_at IS DISTINCT FROM OLD.verified_at THEN
        RAISE EXCEPTION 'verified receipt is immutable' USING ERRCODE='23514';
    END IF;
    IF OLD.attached_at IS NOT NULL AND NEW.attached_at IS DISTINCT FROM OLD.attached_at THEN
        RAISE EXCEPTION 'attached receipt is immutable' USING ERRCODE='23514';
    END IF;
    IF OLD.state='attached' AND NEW.delete_after IS NOT NULL THEN
        RAISE EXCEPTION 'attached private object cannot expire' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER storage_objects_immutable
    BEFORE UPDATE ON storage_objects
    FOR EACH ROW EXECUTE FUNCTION guard_storage_object_immutability();

CREATE FUNCTION guard_attached_storage_delete()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.state='attached' THEN
        RAISE EXCEPTION 'attached private object cannot be deleted'
            USING ERRCODE='23514';
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER storage_objects_attached_retention
    BEFORE DELETE ON storage_objects
    FOR EACH ROW EXECUTE FUNCTION guard_attached_storage_delete();
