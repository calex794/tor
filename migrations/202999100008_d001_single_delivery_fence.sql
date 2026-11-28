-- D-001: exact FREE SINGLE signed delivery lifetime and transaction fence.
-- No seal, S3 mutation, automatic worker or deletion authorization.
-- Older binaries do not register deadlines: any future retirement activation
-- requires a separately verified cutover/drain of all old grant producers.
CREATE TABLE public.d001_free_single_delivery_holds (
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    protected_until TIMESTAMPTZ NOT NULL,
    PRIMARY KEY(chapter_id,version_id),
    FOREIGN KEY(chapter_id,version_id)
        REFERENCES public.free_chapter_storage_objects(chapter_id,version_id)
        ON DELETE RESTRICT
);

COMMENT ON TABLE public.d001_free_single_delivery_holds IS
    'Conservative deadline for signed FREE SINGLE GET, including anonymous and administrator grants. Not a reading right or a complete inventory of in-flight reads.';

-- Called only after successful signing, before the final ACL recheck/return.
-- All admission writers take KEY SHARE on the exact immutable version. A
-- transaction holding the retirement FOR UPDATE fence conflicts with them.
-- Coalesce to minute boundaries (at least five minutes, at most six) to avoid
-- writing a new row version for every request to a popular chapter.
CREATE FUNCTION public.d001_record_free_single_delivery(
    p_chapter BIGINT, p_version BIGINT, p_bucket TEXT,
    p_sha256 BYTEA, p_size BIGINT
) RETURNS BOOLEAN LANGUAGE plpgsql SECURITY DEFINER
SET search_path=pg_catalog,pg_temp AS $$
BEGIN
    PERFORM v.id
      FROM public.chapter_versions v
      JOIN public.free_chapter_storage_objects f
        ON f.chapter_id=v.chapter_id AND f.version_id=v.id
     WHERE v.chapter_id=p_chapter AND v.id=p_version
       AND f.bucket=p_bucket AND f.sha256=p_sha256 AND f.size_bytes=p_size
       AND v.content_sha256=f.sha256
     FOR KEY SHARE OF v;
    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;

    INSERT INTO public.d001_free_single_delivery_holds AS existing
        (chapter_id,version_id,protected_until)
    VALUES(p_chapter,p_version,date_trunc('minute',clock_timestamp())+interval '6 minutes')
    ON CONFLICT(chapter_id,version_id) DO UPDATE
        SET protected_until=EXCLUDED.protected_until
        WHERE existing.protected_until<EXCLUDED.protected_until;
    RETURN TRUE;
END;
$$;

REVOKE ALL ON FUNCTION public.d001_record_free_single_delivery(bigint,bigint,text,bytea,bigint)
    FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname='tor_app') THEN
        REVOKE INSERT,UPDATE,DELETE,TRUNCATE
            ON public.d001_free_single_delivery_holds FROM tor_app;
        GRANT SELECT ON public.d001_free_single_delivery_holds TO tor_app;
        GRANT EXECUTE ON FUNCTION public.d001_record_free_single_delivery(bigint,bigint,text,bytea,bigint)
            TO tor_app;
    END IF;
END $$;

-- This is deliberately only a transaction-scoped probe, not a durable seal.
-- Book -> Chapter -> Version order matches author publication; NOWAIT means
-- no waiting while holding a partial set of locks. Once the caller ends the
-- transaction, a new reader pin or delivery may immediately be admitted.
-- Do not perform object deletion using this result.
CREATE FUNCTION public.d001_try_lock_free_single_retirement(
    p_chapter BIGINT, p_version BIGINT
) RETURNS BOOLEAN LANGUAGE plpgsql SECURITY INVOKER AS $$
DECLARE
    book_id_value BIGINT;
BEGIN
    IF current_setting('transaction_isolation')<>'read committed' THEN
        RETURN FALSE;
    END IF;

    SELECT c.book_id INTO book_id_value
        FROM public.chapters c WHERE c.id=p_chapter;
    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;

    PERFORM b.id FROM public.books b
        WHERE b.id=book_id_value FOR UPDATE NOWAIT;
    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;

    PERFORM c.id FROM public.chapters c
        WHERE c.id=p_chapter AND c.book_id=book_id_value FOR UPDATE NOWAIT;
    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;

    PERFORM v.id FROM public.chapter_versions v
        WHERE v.chapter_id=p_chapter AND v.id=p_version FOR UPDATE NOWAIT;
    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;

    -- A separate, fresh statement after the row locks. Existing references
    -- that use the FK have either committed before this check or will wait.
    RETURN EXISTS(
        SELECT 1 FROM public.d001_free_single_retirement_preflight f
        JOIN public.d001_free_single_retirement_observations o
          USING(chapter_id,version_id)
        WHERE f.chapter_id=p_chapter AND f.version_id=p_version
          AND NOT EXISTS(
              SELECT 1 FROM public.d001_free_single_delivery_holds h
              WHERE h.chapter_id=p_chapter AND h.version_id=p_version
                AND h.protected_until>clock_timestamp()
          )
    );
EXCEPTION WHEN lock_not_available THEN
    RETURN FALSE;
END;
$$;

COMMENT ON FUNCTION public.d001_try_lock_free_single_retirement(bigint,bigint) IS
    'Transaction-only NOWAIT probe for known reference writers and recorded signed SINGLE deadlines. NOT durable admission closure, S3 DELETE permission, soft-reference proof or independent DR.';
