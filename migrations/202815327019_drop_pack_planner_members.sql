-- DROP-R4c: admit at most one active offline PACK per book and remember
-- the exact immutable edition membership used by each approved job.
-- No active reader placement, real S3 object, or historical SINGLE is changed.
CREATE UNIQUE INDEX free_pack_jobs_one_live_book_idx ON free_pack_jobs(book_id)
    WHERE status IN ('pending','running');

CREATE TABLE free_pack_job_members (
    job_id BIGINT NOT NULL REFERENCES free_pack_jobs(id) ON DELETE RESTRICT,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    PRIMARY KEY (job_id,chapter_id,version_id),
    FOREIGN KEY (chapter_id,version_id)
        REFERENCES chapter_versions(chapter_id,id) ON DELETE RESTRICT
);
CREATE INDEX free_pack_job_members_source_idx
    ON free_pack_job_members(chapter_id,version_id);
CREATE TRIGGER drop_pack_job_members_immutable
    BEFORE UPDATE OR DELETE ON free_pack_job_members FOR EACH ROW
    EXECUTE FUNCTION reject_free_chapter_storage_change();
COMMENT ON TABLE free_pack_job_members IS
    'DROP-R4c: append-only approved edition membership for deduplicated offline PACK planning; not entitlement or active reading.';
