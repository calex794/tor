-- C-06 imported from PR #231: first protected-grant issuance evidence for a direct paid V1.
-- Unlike reader progress this requires a real pinned purchase fact. It does
-- NOT prove a browser download, authorize a refund or move wallet funds.
CREATE TABLE chapter_purchase_first_exposures (
    purchase_id BIGINT PRIMARY KEY,
    user_id BIGINT NOT NULL,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    object_kind TEXT NOT NULL CHECK (object_kind IN ('chapter','asset')),
    granted_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (purchase_id,user_id,chapter_id,version_id)
        REFERENCES chapter_purchase_facts (id,user_id,chapter_id,version_id)
        ON DELETE RESTRICT
);

CREATE TRIGGER c06_direct_first_exposures_immutable
    BEFORE UPDATE OR DELETE ON chapter_purchase_first_exposures
    FOR EACH ROW EXECUTE FUNCTION reject_purchase_history_mutation();

-- Only purchases committed by post-cutover, instrumented checkouts can
-- claim complete tracking of first protected grant. Historical NULL = UNKNOWN.
ALTER TABLE chapter_purchase_facts
  ADD COLUMN first_grant_tracking_version TEXT
    CHECK (first_grant_tracking_version IS NULL OR
           first_grant_tracking_version='c06-first-issue-v1');
