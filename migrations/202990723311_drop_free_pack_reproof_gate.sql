-- DROP-07: serialize activation read admission and source PACK sealing.
-- A read reserves the full recovery window even if its process disappears.
ALTER TABLE public.free_pack_placement_gates ADD COLUMN reproof_until TIMESTAMPTZ;

CREATE INDEX free_pack_jobs_registered_pack_idx ON public.free_pack_jobs(pack_id)
    WHERE pack_id IS NOT NULL;

CREATE FUNCTION public.reserve_free_pack_activation_reproof(
    build_job BIGINT, build_epoch BIGINT, fingerprint BYTEA, activation_epoch BIGINT
) RETURNS BOOLEAN LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
BEGIN
    UPDATE public.free_pack_placement_gates gate
       SET reproof_until=GREATEST(gate.reproof_until,clock_timestamp()+interval '5 minutes')
      FROM public.free_pack_jobs j
     WHERE j.id=build_job AND j.lease_epoch=build_epoch AND j.status='succeeded'
       AND j.approved_fingerprint=fingerprint
       AND gate.pack_id=j.pack_id AND gate.sealed_at IS NULL
       AND (activation_epoch IS NULL OR EXISTS (
           SELECT 1 FROM public.free_pack_activation_jobs a WHERE a.pack_job_id=j.id
             AND a.status='running' AND a.lease_epoch=activation_epoch
             AND a.locked_until>clock_timestamp()
       ));
    RETURN FOUND;
END;
$$;

CREATE FUNCTION public.guard_free_pack_activation_queue_gate()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
DECLARE
    pack_open BOOLEAN;
BEGIN
    -- The same gate row serializes a newly queued activation with a seal.
    SELECT g.sealed_at IS NULL INTO pack_open
      FROM public.free_pack_jobs j JOIN public.free_pack_placement_gates g ON g.pack_id=j.pack_id
     WHERE j.id=NEW.pack_job_id AND j.status='succeeded' FOR SHARE OF g;
    IF pack_open IS DISTINCT FROM TRUE THEN
        RAISE EXCEPTION 'DROP: PACK is closed for activation work' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER drop_free_pack_activation_queue_gate
    BEFORE INSERT OR UPDATE OF status ON public.free_pack_activation_jobs
    FOR EACH ROW WHEN (NEW.status IN ('pending','running'))
    EXECUTE FUNCTION public.guard_free_pack_activation_queue_gate();

CREATE FUNCTION public.guard_free_pack_seal_background_reads()
RETURNS TRIGGER LANGUAGE plpgsql SET search_path=pg_catalog,pg_temp AS $$
BEGIN
    IF OLD.reproof_until>clock_timestamp() OR EXISTS (
        SELECT 1 FROM public.free_pack_jobs j JOIN public.free_pack_activation_jobs a ON a.pack_job_id=j.id
         WHERE j.pack_id=OLD.pack_id AND a.status IN ('pending','running')
    ) THEN
        RAISE EXCEPTION 'DROP: PACK has outstanding activation work or read time' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER drop_free_pack_seal_background_reads
    BEFORE UPDATE OF sealed_at ON public.free_pack_placement_gates
    FOR EACH ROW WHEN (OLD.sealed_at IS NULL AND NEW.sealed_at IS NOT NULL)
    EXECUTE FUNCTION public.guard_free_pack_seal_background_reads();

REVOKE ALL ON FUNCTION public.reserve_free_pack_activation_reproof(bigint,bigint,bytea,bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.guard_free_pack_activation_queue_gate() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.guard_free_pack_seal_background_reads() FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname='tor_app') THEN
        GRANT EXECUTE ON FUNCTION public.reserve_free_pack_activation_reproof(bigint,bigint,bytea,bigint) TO tor_app;
    END IF;
END $$;
