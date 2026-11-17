-- DROP-05b: durable, lease-fenced activation admission for newly verified PACK.
-- Additive repository-only schema: NOT a production migration / historical backfill.
-- The original R4a finish transaction enqueues on its status change, whether
-- the cutover worker is enabled or not. A crash cannot lose this activation.
CREATE TABLE free_pack_activation_jobs (
    pack_job_id BIGINT PRIMARY KEY REFERENCES free_pack_jobs(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'pending'
      CHECK (status IN ('pending','running','succeeded','failed')),
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 8),
    locked_until TIMESTAMPTZ,
    retry_after TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    completed_at TIMESTAMPTZ,
    failure_code TEXT CHECK (failure_code IS NULL OR octet_length(failure_code) BETWEEN 1 AND 48),
    CHECK ((status='running') = (locked_until IS NOT NULL)),
    CHECK (status<>'running' OR (lease_epoch>0 AND attempts>0)),
    CHECK ((status IN ('succeeded','failed')) = (completed_at IS NOT NULL))
);
CREATE INDEX free_pack_activation_ready_idx
    ON free_pack_activation_jobs(status,retry_after,locked_until,pack_job_id)
    WHERE status IN ('pending','running');

CREATE TRIGGER free_pack_activation_identity_immutable
    BEFORE UPDATE OF pack_job_id OR DELETE ON free_pack_activation_jobs
    FOR EACH ROW EXECUTE FUNCTION reject_free_chapter_storage_change();

CREATE FUNCTION enqueue_succeeded_free_pack_activation()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.status='succeeded' AND OLD.status IS DISTINCT FROM 'succeeded' THEN
        -- R4a guarantees pack_id, exact immutable membership and receipt.
        -- This INSERT commits atomically with finished physical candidate.
        IF NEW.pack_id IS NULL THEN
            RAISE EXCEPTION 'DROP-05b: cannot enqueue pack without physical receipt'
                USING ERRCODE='23514';
        END IF;
        INSERT INTO free_pack_activation_jobs(pack_job_id)
        VALUES(NEW.id) ON CONFLICT (pack_job_id) DO NOTHING;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER free_pack_activation_enqueue
    AFTER UPDATE OF status ON free_pack_jobs
    FOR EACH ROW EXECUTE FUNCTION enqueue_succeeded_free_pack_activation();

COMMENT ON TABLE free_pack_activation_jobs IS
    'DROP-05b: durable candidate-to-active CAS queue; no active change until independent G proofs and B fenced activation; default worker OFF.';
