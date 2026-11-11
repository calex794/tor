-- S059: server-issued causal metadata, not a user/account/device identifier.
-- Existing jobs remain uncorrelated. Do not backfill or rewrite their origin.
ALTER TABLE chapter_import_jobs
    ADD COLUMN origin_trace_id bytea;

ALTER TABLE chapter_import_jobs
    ADD CONSTRAINT chapter_import_trace_origin_shape CHECK (
        origin_trace_id IS NULL OR (
            octet_length(origin_trace_id) = 16
            AND origin_trace_id <> decode(repeat('00', 16), 'hex')
        )
    );

CREATE FUNCTION tor_import_trace_origin_immutable()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.origin_trace_id IS DISTINCT FROM OLD.origin_trace_id THEN
        RAISE EXCEPTION 'chapter import trace origin is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER chapter_import_trace_origin_immutable
    BEFORE UPDATE OF origin_trace_id ON chapter_import_jobs
    FOR EACH ROW EXECUTE FUNCTION tor_import_trace_origin_immutable();
