-- D-001: persistent exact FREE SINGLE admission closure.
-- No invocation in runtime, no object deletion or changes to immutable receipts.
-- The privileged closure function is only a prerequisite for future retirement:
-- do not call on a live deployment until cutover/drain, independent DR and
-- the separately approved administrator history/restore contract are proven.
CREATE TABLE public.d001_free_single_admission_gates (
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL UNIQUE,
    sealed_at TIMESTAMPTZ,
    PRIMARY KEY(chapter_id,version_id),
    FOREIGN KEY(chapter_id,version_id)
        REFERENCES public.free_chapter_storage_objects(chapter_id,version_id)
        ON DELETE RESTRICT
);

CREATE FUNCTION public.d001_register_free_single_gate()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path=pg_catalog,pg_temp AS $$
BEGIN
    INSERT INTO public.d001_free_single_admission_gates(chapter_id,version_id)
    VALUES(NEW.chapter_id,NEW.version_id);
    RETURN NEW;
END;
$$;

-- Install before the backfill: concurrent receipt inserts cannot miss the gate.
CREATE TRIGGER d001_register_free_single_gate
    AFTER INSERT ON public.free_chapter_storage_objects FOR EACH ROW
    EXECUTE FUNCTION public.d001_register_free_single_gate();

INSERT INTO public.d001_free_single_admission_gates(chapter_id,version_id)
SELECT chapter_id,version_id FROM public.free_chapter_storage_objects;

-- Guard even if a deployment script later reapplies broad table DML grants.
-- SECURITY INVOKER is intentional: only the table owner may change the gate.
CREATE FUNCTION public.d001_protect_free_single_gate()
RETURNS TRIGGER LANGUAGE plpgsql SET search_path=pg_catalog,pg_temp AS $$
BEGIN
    IF current_user<>pg_get_userbyid(
        (SELECT relowner FROM pg_class WHERE oid=TG_RELID)
    ) THEN
        RAISE EXCEPTION 'D001: gate is owner managed' USING ERRCODE='42501';
    END IF;
    IF TG_OP='DELETE' OR TG_OP='TRUNCATE' THEN
        RAISE EXCEPTION 'D001: gate history is immutable' USING ERRCODE='23514';
    END IF;
    IF TG_OP='UPDATE' THEN
        IF OLD.sealed_at IS NOT NULL OR NEW.sealed_at IS NULL
            OR NEW.chapter_id<>OLD.chapter_id OR NEW.version_id<>OLD.version_id
        THEN
            RAISE EXCEPTION 'D001: gate may only advance open to sealed'
                USING ERRCODE='23514';
        END IF;
    ELSIF NEW.sealed_at IS NOT NULL THEN
        RAISE EXCEPTION 'D001: new receipts must start open' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER d001_protect_free_single_gate
    BEFORE INSERT OR UPDATE OR DELETE ON public.d001_free_single_admission_gates
    FOR EACH ROW EXECUTE FUNCTION public.d001_protect_free_single_gate();
CREATE TRIGGER d001_protect_free_single_gate_truncate
    BEFORE TRUNCATE ON public.d001_free_single_admission_gates
    FOR EACH STATEMENT EXECUTE FUNCTION public.d001_protect_free_single_gate();

-- All existing exact-version obligation writers share a row with sealing.
-- Version ids are globally unique. Non-SINGLE legacy and PACK-only editions
-- have no gate, and retain their existing behavior. A missing gate for a
-- registered SINGLE fails closed. SELECT FOR SHARE on the updated gate also
-- makes old REPEATABLE READ snapshots fail rather than miss a committed seal.
CREATE FUNCTION public.d001_guard_single_version_admission()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path=pg_catalog,pg_temp AS $$
DECLARE
    column_name TEXT;
    edition BIGINT;
    is_open BOOLEAN;
BEGIN
    FOREACH column_name IN ARRAY TG_ARGV LOOP
        edition := (to_jsonb(NEW)->>column_name)::BIGINT;
        IF edition IS NULL THEN
            CONTINUE;
        END IF;
        SELECT g.sealed_at IS NULL INTO is_open
          FROM public.d001_free_single_admission_gates g
         WHERE g.version_id=edition FOR SHARE;
        IF FOUND THEN
            IF NOT is_open THEN
                RAISE EXCEPTION 'D001: sealed SINGLE cannot acquire new obligations'
                    USING ERRCODE='23514';
            END IF;
        ELSIF EXISTS(
            SELECT 1 FROM public.free_chapter_storage_objects f
             WHERE f.version_id=edition
        ) THEN
            RAISE EXCEPTION 'D001: missing SINGLE admission gate'
                USING ERRCODE='23514';
        END IF;
    END LOOP;
    RETURN NEW;
END;
$$;

CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.chapter_publications
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.chapter_working_drafts
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.chapter_scheduled_releases
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.chapter_purchase_facts
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.reader_saved_free_chapters
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.reader_free_opened_editions
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.reader_free_open_intents
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.book_layer_free_archive_versions
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.reader_book_positions
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.discussion_anchors
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.discussion_comments
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id,resolved_version_id ON public.text_issues
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id','resolved_version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF resolved_version_id ON public.text_issue_status_events
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('resolved_version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.social_free_comment_reports
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.social_paid_comment_reports
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF target_version_id ON public.finance_direct_swap_commands
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('target_version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.chapter_encrypted_release_events
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.chapter_version_assets
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.free_pack_job_members
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.free_edition_storage_generations
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.free_edition_active_storage
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.free_chapter_import_reservations
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');
CREATE TRIGGER d001_single_admission_guard
    BEFORE INSERT OR UPDATE OF version_id ON public.d001_free_single_delivery_holds
    FOR EACH ROW EXECUTE FUNCTION public.d001_guard_single_version_admission('version_id');

CREATE OR REPLACE FUNCTION public.d001_record_free_single_delivery(
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

    PERFORM g.version_id
      FROM public.d001_free_single_admission_gates g
     WHERE g.chapter_id=p_chapter AND g.version_id=p_version
       AND g.sealed_at IS NULL FOR SHARE;
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


CREATE OR REPLACE VIEW d001_free_single_retirement_preflight AS
SELECT f.chapter_id, f.version_id, f.bucket, f.object_key, f.sha256, f.size_bytes
FROM public.free_chapter_storage_objects f
JOIN public.d001_free_single_admission_gates gate
  ON gate.chapter_id=f.chapter_id AND gate.version_id=f.version_id
 AND gate.sealed_at IS NULL
JOIN chapter_versions v
  ON v.chapter_id=f.chapter_id AND v.id=f.version_id
JOIN chapters c ON c.id=f.chapter_id
JOIN books b ON b.id=c.book_id
JOIN chapter_publications p ON p.chapter_id=c.id
JOIN chapter_versions head ON head.chapter_id=c.id AND head.id=p.version_id
JOIN d001_chapter_materialization_roles roles
  ON roles.chapter_id=f.chapter_id AND roles.version_id=f.version_id
WHERE NOT roles.is_logically_retained
  AND NOT roles.has_cursor_reference
  AND c.access_level='free' AND c.price_amount=0
  AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
  AND b.visibility='public' AND b.visibility_status='active'
  AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
  AND NOT b.admin_hold AND NOT b.is_deleted
  AND v.version_no<head.version_no
  AND v.body_markdown IS NULL AND v.storage_object_id IS NULL
  AND v.content_sha256=f.sha256
  -- Even expired but not yet cleaned intents block observation: a confirmation
  -- may have passed its earlier expiry check before taking its own locks.
  AND NOT EXISTS (
      SELECT 1 FROM reader_free_open_intents i
      WHERE i.chapter_id=f.chapter_id AND i.version_id=f.version_id
  )
  -- Keep SINGLEs involved in any PACK history/job, including rollback sources.
  AND NOT EXISTS (
      SELECT 1 FROM free_edition_storage_generations g
      WHERE g.chapter_id=f.chapter_id AND g.version_id=f.version_id
        AND g.placement_kind='pack'
  )
  AND NOT EXISTS (
      SELECT 1 FROM free_pack_job_members m
      WHERE m.chapter_id=f.chapter_id AND m.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM free_chapter_import_reservations r
      WHERE r.chapter_id=f.chapter_id AND r.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM chapter_scheduled_releases s
      WHERE s.chapter_id=f.chapter_id AND s.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM social_free_comment_reports r
      WHERE r.chapter_id=f.chapter_id AND r.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM social_paid_comment_reports r
      WHERE r.chapter_id=f.chapter_id AND r.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM finance_direct_swap_commands s
      WHERE s.target_chapter_id=f.chapter_id AND s.target_version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM chapter_encrypted_release_events e
      WHERE e.chapter_id=f.chapter_id AND e.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM chapter_version_assets a
      WHERE a.chapter_id=f.chapter_id AND a.version_id=f.version_id
  );


-- Only the migration/table-owner role can commit closure. No API/worker
-- calls this function. NOWAIT keeps the probe from deadlocking a writer that
-- holds the gate before its FK check. The final check is after both locks.
CREATE FUNCTION public.d001_seal_free_single_for_retirement(
    p_chapter BIGINT, p_version BIGINT
) RETURNS BOOLEAN LANGUAGE plpgsql SECURITY INVOKER AS $$
DECLARE
    is_sealed BOOLEAN;
BEGIN
    IF current_setting('transaction_isolation')<>'read committed' THEN
        RETURN FALSE;
    END IF;
    SELECT sealed_at IS NOT NULL INTO is_sealed
      FROM public.d001_free_single_admission_gates
     WHERE chapter_id=p_chapter AND version_id=p_version;
    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;
    IF is_sealed THEN
        RETURN TRUE;
    END IF;
    IF NOT public.d001_try_lock_free_single_retirement(p_chapter,p_version) THEN
        RETURN FALSE;
    END IF;
    SELECT sealed_at IS NOT NULL INTO is_sealed
      FROM public.d001_free_single_admission_gates
     WHERE chapter_id=p_chapter AND version_id=p_version FOR UPDATE NOWAIT;
    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;
    IF is_sealed THEN
        RETURN TRUE;
    END IF;
    IF NOT public.d001_try_lock_free_single_retirement(p_chapter,p_version) THEN
        RETURN FALSE;
    END IF;
    UPDATE public.d001_free_single_admission_gates SET sealed_at=clock_timestamp()
     WHERE chapter_id=p_chapter AND version_id=p_version;
    RETURN FOUND;
EXCEPTION WHEN lock_not_available THEN
    RETURN FALSE;
END;
$$;

REVOKE ALL ON TABLE public.d001_free_single_admission_gates FROM PUBLIC;
REVOKE ALL ON FUNCTION public.d001_register_free_single_gate() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.d001_protect_free_single_gate() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.d001_guard_single_version_admission() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.d001_seal_free_single_for_retirement(bigint,bigint) FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname='tor_app') THEN
        REVOKE ALL ON TABLE public.d001_free_single_admission_gates FROM tor_app;
        GRANT SELECT ON public.d001_free_single_admission_gates TO tor_app;
        REVOKE ALL ON FUNCTION public.d001_register_free_single_gate() FROM tor_app;
        REVOKE ALL ON FUNCTION public.d001_protect_free_single_gate() FROM tor_app;
        REVOKE ALL ON FUNCTION public.d001_guard_single_version_admission() FROM tor_app;
        REVOKE ALL ON FUNCTION public.d001_seal_free_single_for_retirement(bigint,bigint)
            FROM tor_app;
    END IF;
END $$;

COMMENT ON TABLE public.d001_free_single_admission_gates IS
    'Durable one-way exact SINGLE admission closure. Not proof of independent backup, provider deletion or all soft references. Restore/reopen requires a separate contract.';
COMMENT ON FUNCTION public.d001_seal_free_single_for_retirement(bigint,bigint) IS
    'Owner-only uninvoked admission closure prerequisite. Never an S3 DELETE permit; no live call until explicit activation, grant drain, admin archive/restore and independent DR.';
