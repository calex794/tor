-- DROP-07i: append-only request before remote effects, receipt afterwards.
CREATE TABLE free_storage_quarantine_delete_requests (
    bucket TEXT NOT NULL,
    object_key TEXT NOT NULL,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(bucket,object_key),
    FOREIGN KEY(bucket,object_key) REFERENCES free_storage_quarantine_rechecks(bucket,object_key)
);
CREATE TRIGGER free_storage_quarantine_delete_requests_immutable
    BEFORE UPDATE OR DELETE ON free_storage_quarantine_delete_requests FOR EACH ROW
    EXECUTE FUNCTION reject_free_chapter_storage_change();

CREATE TABLE free_storage_quarantine_delete_receipts (
    bucket TEXT NOT NULL,
    object_key TEXT NOT NULL,
    verified_absent_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(bucket,object_key),
    FOREIGN KEY(bucket,object_key) REFERENCES free_storage_quarantine_delete_requests(bucket,object_key)
);
CREATE TRIGGER free_storage_quarantine_delete_receipts_immutable
    BEFORE UPDATE OR DELETE ON free_storage_quarantine_delete_receipts FOR EACH ROW
    EXECUTE FUNCTION reject_free_chapter_storage_change();
