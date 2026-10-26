-- B-06: a working version is never a public chapter pointer.
-- In a published book, chapters.visibility='draft' AND a working row keep
-- the body invisible to every public reader/TOC/access SQL query.
CREATE TABLE chapter_working_drafts (
    chapter_id BIGINT PRIMARY KEY REFERENCES chapters(id) ON DELETE CASCADE,
    version_id BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (chapter_id, version_id)
        REFERENCES chapter_versions(chapter_id, id) ON DELETE RESTRICT
);

-- No data backfill: legacy private books use the existing chapter_publications
-- pointer while the BOOK is hidden. Public-book drafts use this table only.
