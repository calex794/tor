CREATE TABLE free_pack_rollback_receipts (
    job_id BIGINT PRIMARY KEY REFERENCES free_pack_jobs(id) ON DELETE RESTRICT,
    pack_id BIGINT NOT NULL REFERENCES free_storage_packs(id) ON DELETE RESTRICT,
    build_epoch BIGINT NOT NULL CHECK (build_epoch > 0),
    operator_role NAME NOT NULL DEFAULT session_user,
    committed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

CREATE TRIGGER free_pack_rollback_receipts_immutable
    BEFORE UPDATE OR DELETE ON free_pack_rollback_receipts FOR EACH ROW
    EXECUTE FUNCTION reject_free_chapter_storage_change();
