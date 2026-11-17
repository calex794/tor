-- DROP-04d: one immutable storage decision per approved leased S023 job.
-- Additive, repository-only; never applied by HTTP startup or on the live VPS.
CREATE TABLE free_chapter_import_transports (
    job_id BIGINT PRIMARY KEY REFERENCES chapter_import_jobs(id) ON DELETE RESTRICT,
    actor_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    command_key TEXT NOT NULL,
    approved_fingerprint BYTEA NOT NULL CHECK (octet_length(approved_fingerprint)=32),
    chapter_count INTEGER NOT NULL CHECK (chapter_count BETWEEN 1 AND 500),
    transport TEXT NOT NULL CHECK (transport IN ('single','pack')),
    selected_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(actor_user_id,command_key)
);
CREATE TRIGGER free_import_transport_immutable
    BEFORE UPDATE OR DELETE ON free_chapter_import_transports FOR EACH ROW
    EXECUTE FUNCTION reject_free_chapter_storage_change();

CREATE FUNCTION guard_free_import_transport()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM chapter_import_jobs j
        WHERE j.id=NEW.job_id AND j.actor_user_id=NEW.actor_user_id
          AND j.book_id=NEW.book_id AND j.command_key=NEW.command_key
          AND j.approved_fingerprint=NEW.approved_fingerprint
          AND j.expected_chapter_count=NEW.chapter_count
          AND j.status='running' AND j.locked_until>clock_timestamp()
    ) THEN
        RAISE EXCEPTION 'DROP: transport requires exact running approved job' USING ERRCODE='23514';
    END IF;
    IF NEW.transport='single' AND EXISTS (
        SELECT 1 FROM free_pack_direct_import_stages s WHERE s.job_id=NEW.job_id
    ) THEN
        RAISE EXCEPTION 'DROP: staged PACK cannot fall back to SINGLE' USING ERRCODE='23514';
    END IF;
    IF NEW.transport='pack' AND EXISTS (
        SELECT 1 FROM free_chapter_import_reservations r
        JOIN free_chapter_import_uploads u ON u.version_id=r.version_id
        WHERE r.user_id=NEW.actor_user_id AND r.command_key=NEW.command_key
    ) THEN
        RAISE EXCEPTION 'DROP: existing SINGLE checkpoint cannot become PACK' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER free_import_transport_exact_job
    BEFORE INSERT ON free_chapter_import_transports FOR EACH ROW
    EXECUTE FUNCTION guard_free_import_transport();

CREATE FUNCTION guard_free_import_single_checkpoint()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM free_chapter_import_reservations r
        JOIN free_chapter_import_transports t
          ON t.actor_user_id=r.user_id AND t.command_key=r.command_key
        WHERE r.version_id=NEW.version_id AND t.transport='pack'
    ) OR EXISTS (
        SELECT 1 FROM free_chapter_import_reservations r
        JOIN chapter_import_jobs j ON j.actor_user_id=r.user_id AND j.command_key=r.command_key
        JOIN free_pack_direct_import_stages s ON s.job_id=j.id
        WHERE r.version_id=NEW.version_id
    ) THEN
        RAISE EXCEPTION 'DROP: PACK job cannot checkpoint SINGLE' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER free_import_single_transport_guard
    BEFORE INSERT ON free_chapter_import_uploads FOR EACH ROW
    EXECUTE FUNCTION guard_free_import_single_checkpoint();

CREATE FUNCTION guard_free_import_pack_checkpoint()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM free_chapter_import_transports t
        WHERE t.job_id=NEW.job_id AND t.transport='single'
    ) THEN
        RAISE EXCEPTION 'DROP: SINGLE job cannot checkpoint PACK' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER free_import_pack_transport_guard
    BEFORE INSERT ON free_pack_direct_import_stages FOR EACH ROW
    EXECUTE FUNCTION guard_free_import_pack_checkpoint();
