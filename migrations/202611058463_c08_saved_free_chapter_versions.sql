-- C-08 / BR-060: immutable reader-owned saved-free version references.
-- Deliberately independent of reader_library_items (Book bookmark) and
-- library_purchased_chapters (verified paid entitlement).
-- This migration adds no public read or deletion endpoints. A future C-owned
-- repository must insert ONLY currently published/free chapter versions within
-- a live actor/Book permission transaction; never trust a client version id.
CREATE TABLE reader_saved_free_chapters (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    saved_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, chapter_id),
    FOREIGN KEY (chapter_id, version_id)
        REFERENCES chapter_versions(chapter_id, id) ON DELETE RESTRICT
);

-- Future keyset chapter listing is scoped by the owner and never scans all users.
CREATE INDEX reader_saved_free_chapters_owner_recent_idx
    ON reader_saved_free_chapters (user_id, saved_at DESC, chapter_id DESC);

-- A later ChapterVersion publication must never repoint an earlier saved free
-- entry. Physical erasure of a pinned version is blocked; legal/admin denial is
-- enforced on read and a separately authorized erasure process must clear pins.
CREATE FUNCTION reject_saved_free_chapter_version_update()
RETURNS trigger LANGUAGE plpgsql AS $saved_free_immutable$
BEGIN
    RAISE EXCEPTION 'saved free chapter version is immutable'
        USING ERRCODE = '23514';
END;
$saved_free_immutable$;

CREATE TRIGGER saved_free_chapter_no_mutation
    BEFORE UPDATE ON reader_saved_free_chapters
    FOR EACH ROW EXECUTE FUNCTION reject_saved_free_chapter_version_update();
