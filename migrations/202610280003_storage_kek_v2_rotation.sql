-- S3-34: permit versioned KEK metadata while preserving immutable ciphertext.
-- No background rotation, no production secrets, and no privilege for tor_app.
ALTER TABLE storage_objects DROP CONSTRAINT storage_objects_kek_id_check;
ALTER TABLE storage_objects
    ADD CONSTRAINT storage_objects_kek_id_check
    CHECK (kek_id IN ('content-kek-v1','content-kek-v2'));

-- G's receipt and S3 identity remain immutable even for the operator.
-- Only tor_migrator may move an ATTACHED wrapper once from V1 to V2;
-- all other updates retain the original strict state-transition contract.
CREATE OR REPLACE FUNCTION guard_storage_object_immutability()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(
        NEW.id,NEW.object_kind,NEW.bucket,NEW.object_key,NEW.size_bytes,
        NEW.sha256_ciphertext,NEW.plaintext_sha256,
        NEW.object_nonce,NEW.object_aad,NEW.codec
    ) IS DISTINCT FROM ROW(
        OLD.id,OLD.object_kind,OLD.bucket,OLD.object_key,OLD.size_bytes,
        OLD.sha256_ciphertext,OLD.plaintext_sha256,
        OLD.object_nonce,OLD.object_aad,OLD.codec
    ) THEN
        RAISE EXCEPTION 'immutable private storage object'
            USING ERRCODE='23514';
    END IF;

    IF ROW(NEW.encrypted_dek,NEW.wrap_nonce,NEW.kek_id)
        IS DISTINCT FROM ROW(OLD.encrypted_dek,OLD.wrap_nonce,OLD.kek_id) THEN
        IF NOT (
            current_user='tor_migrator'
            AND OLD.state='attached' AND NEW.state='attached'
            AND OLD.kek_id='content-kek-v1'
            AND NEW.kek_id='content-kek-v2'
            AND NEW.encrypted_dek IS DISTINCT FROM OLD.encrypted_dek
            AND NEW.wrap_nonce IS DISTINCT FROM OLD.wrap_nonce
            AND ROW(
                NEW.verified_at,NEW.attached_at,NEW.delete_after,
                NEW.gc_lease_epoch,NEW.gc_attempts,NEW.gc_locked_until,
                NEW.gc_retry_after,NEW.gc_error_code
            ) IS NOT DISTINCT FROM ROW(
                OLD.verified_at,OLD.attached_at,OLD.delete_after,
                OLD.gc_lease_epoch,OLD.gc_attempts,OLD.gc_locked_until,
                OLD.gc_retry_after,OLD.gc_error_code
            )
        ) THEN
            RAISE EXCEPTION 'immutable private storage key wrapper'
                USING ERRCODE='23514';
        END IF;
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

-- An operator supplies a G-verified V2 wrapped DEK. Compare against the exact
-- old V1 envelope and state to prevent lost updates, rewinds and stale retries.
-- This function cannot establish cryptographic validity by itself; callers
-- must use the authenticated Rust rewrap primitive after a verified backup.
CREATE FUNCTION rewrap_attached_storage_v1_to_v2(
    object UUID, old_wrapped BYTEA, old_nonce BYTEA,
    new_wrapped BYTEA, new_nonce BYTEA
) RETURNS BOOLEAN LANGUAGE plpgsql SECURITY INVOKER
SET search_path=pg_catalog,public AS $$
DECLARE updated INTEGER;
BEGIN
    IF current_user <> 'tor_migrator' THEN
        RAISE EXCEPTION 'operator-only storage KEK rotation' USING ERRCODE='42501';
    END IF;
    IF object IS NULL
        OR old_wrapped IS NULL OR octet_length(old_wrapped)<>48
        OR old_nonce IS NULL OR octet_length(old_nonce)<>12
        OR new_wrapped IS NULL OR octet_length(new_wrapped)<>48
        OR new_nonce IS NULL OR octet_length(new_nonce)<>12
        OR old_wrapped=new_wrapped OR old_nonce=new_nonce
    THEN
        RAISE EXCEPTION 'invalid storage rewrap envelope' USING ERRCODE='23514';
    END IF;
    UPDATE public.storage_objects
       SET encrypted_dek=new_wrapped, wrap_nonce=new_nonce,
           kek_id='content-kek-v2'
     WHERE id=object
       AND object_kind='chapter'
       AND state='attached'
       AND kek_id='content-kek-v1'
       AND encrypted_dek=old_wrapped AND wrap_nonce=old_nonce;
    GET DIAGNOSTICS updated=ROW_COUNT;
    RETURN updated=1;
END;
$$;
REVOKE ALL ON FUNCTION rewrap_attached_storage_v1_to_v2(
    UUID,BYTEA,BYTEA,BYTEA,BYTEA
) FROM PUBLIC;
-- No GRANT to tor_app. The tor_migrator function owner retains EXECUTE.
