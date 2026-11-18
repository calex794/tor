-- DROP-07: seal the catalog against reusing a replaced PACK. This does not
-- retire either S3 key, change entitlements, or authorize physical deletion.
CREATE TABLE public.free_pack_placement_gates (
    pack_id BIGINT PRIMARY KEY REFERENCES public.free_storage_packs(id) ON DELETE RESTRICT,
    sealed_at TIMESTAMPTZ
);

CREATE FUNCTION public.register_free_pack_placement_gate()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path=pg_catalog,pg_temp AS $$
BEGIN
    INSERT INTO public.free_pack_placement_gates(pack_id) VALUES (NEW.id);
    RETURN NEW;
END;
$$;

-- Install the trigger before the backfill. Its DDL lock excludes concurrent
-- catalog inserts until the migration transaction commits.
CREATE TRIGGER drop_free_pack_placement_gate_register
    AFTER INSERT ON public.free_storage_packs FOR EACH ROW
    EXECUTE FUNCTION public.register_free_pack_placement_gate();
INSERT INTO public.free_pack_placement_gates(pack_id) SELECT id FROM public.free_storage_packs;

CREATE FUNCTION public.guard_free_pack_placement_gate()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path=pg_catalog,pg_temp AS $$
DECLARE
    target_pack BIGINT;
    target_open BOOLEAN;
BEGIN
    IF TG_TABLE_NAME = 'free_edition_storage_generations' THEN
        target_pack := NEW.pack_id;
    ELSE
        SELECT g.pack_id INTO target_pack FROM public.free_edition_storage_generations g
         WHERE (g.chapter_id,g.version_id,g.generation) =
               (NEW.chapter_id,NEW.version_id,NEW.generation);
        IF NOT FOUND THEN
            RAISE EXCEPTION 'DROP: active generation is unavailable' USING ERRCODE='23503';
        END IF;
    END IF;

    -- SINGLE placements have no PACK gate. A missing generation must fail
    -- here, even if a racing insert could later satisfy the foreign key.
    IF target_pack IS NOT NULL THEN
        SELECT sealed_at IS NULL INTO target_open FROM public.free_pack_placement_gates
         WHERE pack_id=target_pack FOR SHARE;
        IF target_open IS DISTINCT FROM TRUE THEN
            RAISE EXCEPTION 'DROP: PACK is closed for new placements' USING ERRCODE='23514';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER drop_free_pack_placement_gate_generation
    BEFORE INSERT ON public.free_edition_storage_generations
    FOR EACH ROW WHEN (NEW.pack_id IS NOT NULL)
    EXECUTE FUNCTION public.guard_free_pack_placement_gate();
CREATE TRIGGER drop_free_pack_placement_gate_activation
    BEFORE INSERT OR UPDATE OF chapter_id,version_id,generation ON public.free_edition_active_storage
    FOR EACH ROW EXECUTE FUNCTION public.guard_free_pack_placement_gate();

CREATE FUNCTION public.seal_replaced_free_pack_placements(source_pack BIGINT)
RETURNS BOOLEAN LANGUAGE plpgsql SET search_path=pg_catalog,pg_temp AS $$
DECLARE
    existing_seal TIMESTAMPTZ;
    source_placements BIGINT;
    source_editions BIGINT;
    locked_editions BIGINT;
BEGIN
    -- The function must see existing writes even if the gate tuple was only
    -- locked, not changed, by the previous transaction.
    IF current_setting('transaction_isolation') <> 'read committed' THEN
        RAISE EXCEPTION 'DROP: PACK sealing requires READ COMMITTED' USING ERRCODE='25001';
    END IF;

    SELECT sealed_at INTO existing_seal FROM public.free_pack_placement_gates
     WHERE pack_id=source_pack FOR NO KEY UPDATE NOWAIT;
    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;
    IF existing_seal IS NOT NULL THEN
        RETURN TRUE;
    END IF;

    SELECT count(*), count(DISTINCT (members.chapter_id,members.version_id))
      INTO source_placements,source_editions FROM (
        SELECT g.chapter_id,g.version_id FROM public.free_edition_storage_generations g
         WHERE g.pack_id=source_pack LIMIT 4097
    ) members;
    IF source_placements NOT BETWEEN 1 AND 4096 THEN
        RETURN FALSE;
    END IF;

    -- Hold the existing edition pointers stable until commit. NOWAIT makes
    -- contention with an in-flight promotion an explicit retry.
    PERFORM a.generation FROM public.free_edition_active_storage a
      JOIN (SELECT DISTINCT chapter_id,version_id FROM public.free_edition_storage_generations
             WHERE pack_id=source_pack) members USING (chapter_id,version_id)
     ORDER BY a.chapter_id,a.version_id FOR SHARE OF a NOWAIT;
    GET DIAGNOSTICS locked_editions = ROW_COUNT;
    IF locked_editions <> source_editions THEN
        RETURN FALSE;
    END IF;

    -- Every source edition needs an active equivalent, even if it currently
    -- has no pin. A future reference can then use the same immutable edition.
    IF EXISTS (
        SELECT 1 FROM public.free_edition_storage_generations old
        JOIN public.free_storage_packs source ON source.id=old.pack_id
        WHERE old.pack_id=source_pack AND NOT EXISTS (
            SELECT 1 FROM public.free_edition_active_storage a
            JOIN public.free_edition_storage_generations g
              USING (chapter_id,version_id,generation)
            JOIN public.free_storage_packs target ON target.id=g.pack_id
            JOIN public.free_pack_placement_gates gate ON gate.pack_id=g.pack_id
            WHERE (a.chapter_id,a.version_id)=(old.chapter_id,old.version_id)
              AND target.id<>source_pack AND target.bucket=source.bucket
              AND gate.sealed_at IS NULL
              AND (g.content_sha256,g.size_bytes)=(old.content_sha256,old.size_bytes)
        )
    ) THEN
        RETURN FALSE;
    END IF;

    UPDATE public.free_pack_placement_gates SET sealed_at=clock_timestamp() WHERE pack_id=source_pack;
    RETURN TRUE;
END;
$$;

REVOKE ALL ON TABLE public.free_pack_placement_gates FROM PUBLIC;
REVOKE ALL ON FUNCTION public.register_free_pack_placement_gate() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.guard_free_pack_placement_gate() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.seal_replaced_free_pack_placements(bigint) FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname='tor_app') THEN
        REVOKE ALL ON TABLE public.free_pack_placement_gates FROM tor_app;
        REVOKE ALL ON FUNCTION public.register_free_pack_placement_gate() FROM tor_app;
        REVOKE ALL ON FUNCTION public.guard_free_pack_placement_gate() FROM tor_app;
        REVOKE ALL ON FUNCTION public.seal_replaced_free_pack_placements(bigint) FROM tor_app;
    END IF;
END $$;

COMMENT ON TABLE public.free_pack_placement_gates IS
    'DROP-07: one-way placement write gate; sealed rows do not confer S3 retirement authority.';
