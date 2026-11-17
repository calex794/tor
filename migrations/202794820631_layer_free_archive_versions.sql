-- D-05 / BR-060: immutable free-publication identity for ordinary Layer archives.
-- Staged in the repository only; never applied to production by this PR.
--
-- Publication snapshot is shared by the book, not written once per Layer buyer.
-- A later V2, a withdrawn publication, or a removed CURRENT Layer version
-- must not change the archived edition already acquired by a paid reader.
CREATE TABLE book_layer_free_archive_versions (
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    request_revision BIGINT NOT NULL CHECK (request_revision > 0),
    chapter_id BIGINT NOT NULL REFERENCES chapters(id) ON DELETE RESTRICT,
    version_id BIGINT NOT NULL REFERENCES chapter_versions(id) ON DELETE RESTRICT,
    captured_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (book_id, request_revision, chapter_id)
);

CREATE INDEX book_layer_free_archive_by_chapter
    ON book_layer_free_archive_versions (chapter_id, request_revision);

-- Do not add public grants, direct purchase rows or saved-free pins here.
-- The reader must independently verify chapter-version membership and D's
-- active historical paid Layer origin before any S3 grant can be issued.
