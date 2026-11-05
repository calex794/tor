-- BR-060 / C-08 / I: preserve exact editions of already-public free chapters
-- when a reader explicitly saves a Book, matching NEwBOO's saved_free items.
-- This is NOT a grant to paid or future chapters, nor the completed deletion flow.
CREATE TABLE reader_saved_free_chapters (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    saved_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, chapter_id),
    CONSTRAINT reader_saved_free_chapter_version_fk
      FOREIGN KEY (chapter_id, version_id)
      REFERENCES chapter_versions (chapter_id, id) ON DELETE RESTRICT
);

-- A pinned edition is immutable; downstream readers must check the live user,
-- book age tier, admin/legal holds and hard-delete flags before returning text.
-- The composite FK intentionally refuses to destructively delete a saved
-- chapter version even if another feature later runs ON DELETE CASCADE.
COMMENT ON TABLE reader_saved_free_chapters IS
    'C-owned per-reader pinned free chapter editions; not a paid entitlement';
