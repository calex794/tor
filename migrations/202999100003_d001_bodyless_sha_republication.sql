-- D-001: a content hash identifies bytes, not a unique S3-only materialization.
--
-- The original unique index prevented a legitimate author A -> B -> A
-- re-publication once bodyless free/paid S3 editions were introduced:
-- immutable version-bound S3 keys/AAD require a fresh physical version_id.
-- Do not reuse a prior S3 edition or alter its exact historical receipts.
--
-- Legacy PostgreSQL-body editing intentionally reuses an existing body row
-- for the same SHA, and retains uniqueness for its legacy materializations.
-- This additive migration preserves those legacy invariants while permitting
-- distinct immutable bodyless attachments with identical source hashes.
DROP INDEX chapter_versions_same_content;

CREATE UNIQUE INDEX chapter_versions_same_content_legacy
    ON chapter_versions(chapter_id,content_sha256)
    WHERE content_sha256 IS NOT NULL
      AND body_markdown IS NOT NULL;

CREATE INDEX chapter_versions_bodyless_content_lookup
    ON chapter_versions(chapter_id,content_sha256)
    WHERE content_sha256 IS NOT NULL
      AND body_markdown IS NULL;

COMMENT ON INDEX chapter_versions_same_content_legacy IS
    'D-001: one legacy PostgreSQL text row per chapter/body hash; bodyless verified S3 materializations may be repeated under distinct immutable version IDs.';

COMMENT ON INDEX chapter_versions_bodyless_content_lookup IS
    'D-001: non-unique index for immutable bodyless re-publications, not a deduplication or deletion contract.';
