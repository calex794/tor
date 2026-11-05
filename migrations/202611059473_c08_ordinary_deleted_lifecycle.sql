-- C-08/BR-060: completed *ordinary* Book removal is not hard deletion.
-- Preserve chapter_version FK pins and purchased versions. No physical DELETE.
-- Application write path must deindex the Book (visibility='draft')
-- in the same transaction as this lifecycle transition.
ALTER TABLE books DROP CONSTRAINT books_lifecycle_status_check;
ALTER TABLE books ADD CONSTRAINT books_lifecycle_status_check
    CHECK (lifecycle_status IN (
        'in_progress', 'completed', 'abandoned', 'on_hiatus',
        'pending_deletion', 'ordinary_deleted'
    ));

ALTER TABLE books ADD COLUMN ordinary_deleted_at TIMESTAMPTZ;
ALTER TABLE books ADD CONSTRAINT books_ordinary_deleted_timestamp_ck
    CHECK ((lifecycle_status='ordinary_deleted') = (ordinary_deleted_at IS NOT NULL));

-- Do not touch books.is_deleted: that is the hard/legal/admin security gate
-- and must never be overloaded to represent ordinary reader-safe removal.
CREATE INDEX books_ordinary_deleted_lookup_idx
    ON books (id)
    WHERE lifecycle_status='ordinary_deleted' AND NOT is_deleted;

COMMENT ON COLUMN books.ordinary_deleted_at IS
    'Final ordinary removal timestamp; never a hard-delete/legal/admin bypass';
