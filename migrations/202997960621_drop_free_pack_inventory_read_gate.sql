-- DROP-07: inventory takes a short hold on the same gate used by activation
-- and sealing. Both keys of a PACK belong to this one gate.
CREATE FUNCTION public.reserve_free_pack_inventory_read(storage_bucket TEXT, storage_key TEXT)
RETURNS TEXT LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
DECLARE
    pack BIGINT;
    sealed BOOLEAN;
BEGIN
    SELECT p.id INTO pack FROM public.free_storage_packs p
      WHERE p.bucket=storage_bucket
        AND (p.object_key=storage_key OR p.manifest_key=storage_key);
    IF NOT FOUND THEN
        RETURN NULL;
    END IF;

    UPDATE public.free_pack_placement_gates gate
       SET reproof_until=GREATEST(gate.reproof_until,clock_timestamp()+interval '1 minute')
     WHERE gate.pack_id=pack AND gate.sealed_at IS NULL;
    IF FOUND THEN
        RETURN 'admitted';
    END IF;

    SELECT gate.sealed_at IS NOT NULL INTO sealed FROM public.free_pack_placement_gates gate
      WHERE gate.pack_id=pack;
    IF sealed THEN
        RETURN 'sealed';
    END IF;
    RETURN NULL;
END;
$$;

REVOKE ALL ON FUNCTION public.reserve_free_pack_inventory_read(text,text) FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname='tor_app') THEN
        GRANT EXECUTE ON FUNCTION public.reserve_free_pack_inventory_read(text,text) TO tor_app;
    END IF;
END $$;
