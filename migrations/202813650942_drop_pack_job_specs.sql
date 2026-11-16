-- DROP-R4b1. Immutable preapproved worker descriptor.
-- No live reader, active-generation or S349 import modification.
CREATE TABLE free_pack_job_specs (
    job_id BIGINT PRIMARY KEY REFERENCES free_pack_jobs(id) ON DELETE RESTRICT,
    spec_text TEXT NOT NULL CHECK (octet_length(spec_text) BETWEEN 2 AND 1048576)
);
CREATE FUNCTION drop_guard_pack_job_spec() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'DROP-R4b: approved pack spec is immutable' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER drop_pack_job_spec_immutable
BEFORE UPDATE OR DELETE ON free_pack_job_specs FOR EACH ROW
EXECUTE FUNCTION drop_guard_pack_job_spec();
CREATE FUNCTION drop_guard_approved_pack_job() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM free_pack_job_specs WHERE job_id=OLD.id) AND (
      NEW.actor_user_id IS DISTINCT FROM OLD.actor_user_id OR
      NEW.book_id IS DISTINCT FROM OLD.book_id OR
      NEW.approved_fingerprint IS DISTINCT FROM OLD.approved_fingerprint OR
      NEW.expected_chapter_count IS DISTINCT FROM OLD.expected_chapter_count OR
      NEW.pack_id IS DISTINCT FROM OLD.pack_id AND OLD.status <> 'running'
    ) THEN
      RAISE EXCEPTION 'DROP-R4b: cannot retarget approved job' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER drop_pack_job_approved_immutable
BEFORE UPDATE ON free_pack_jobs FOR EACH ROW
EXECUTE FUNCTION drop_guard_approved_pack_job();
