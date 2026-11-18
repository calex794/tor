-- FND-01: seed a retained S3-only FREE book before subsequent DROP migrations.
-- Disposable PG18 only; receipt metadata is not evidence of real S3 object bytes.
-- Run after S349 migrations and before DROP physical-placement generations.
DO $fnd01_retained_seed$
DECLARE
    fixture_user BIGINT;
    fixture_book BIGINT;
    fixture_chapter BIGINT;
    v1 BIGINT;
    v2 BIGINT;
    body_v1 TEXT := '# FND01 retained V1';
    body_v2 TEXT := '# FND01 retained V2';
BEGIN
    INSERT INTO users(email_normalized, display_name, password_hash)
    VALUES ('fnd01-upgrade@example.invalid', 'FND01 Reader', '$argon2id$ci-only')
    RETURNING id INTO fixture_user;

    INSERT INTO books(source_identity, target_language, title, visibility, owner_user_id)
    VALUES ('fnd01-retained-s3-book', 'ru', 'FND01 Retained S3 Book', 'public', fixture_user)
    RETURNING id INTO fixture_book;

    INSERT INTO chapters(book_id, chapter_no, title, visibility, access_level, price_amount)
    VALUES (fixture_book, 1, 'Retained chapter', 'public', 'free', 0)
    RETURNING id INTO fixture_chapter;

    INSERT INTO chapter_versions(chapter_id, version_no, body_markdown, content_sha256)
    VALUES (fixture_chapter, 1, NULL, sha256(convert_to(body_v1, 'UTF8')))
    RETURNING id INTO v1;

    INSERT INTO free_chapter_storage_objects
        (chapter_id, version_id, bucket, object_key, sha256, size_bytes)
    VALUES (
        fixture_chapter, v1, 'tor-fnd01-private',
        'private/free-chapters/' || fixture_chapter || '/' || v1 || '.md',
        sha256(convert_to(body_v1, 'UTF8')),
        octet_length(convert_to(body_v1, 'UTF8'))
    );

    INSERT INTO chapter_version_content_shapes
        (chapter_id, version_id, content_sha256,
         block_utf16_lengths, last_readable_block_index, last_readable_char_offset)
    VALUES (
        fixture_chapter, v1, sha256(convert_to(body_v1, 'UTF8')),
        ARRAY[char_length(body_v1)], 1, char_length(body_v1)
    );

    INSERT INTO chapter_publications(chapter_id, version_id)
    VALUES (fixture_chapter, v1);

    INSERT INTO reader_saved_free_chapters(user_id, chapter_id, version_id)
    VALUES (fixture_user, fixture_chapter, v1);

    INSERT INTO chapter_versions(chapter_id, version_no, body_markdown, content_sha256)
    VALUES (fixture_chapter, 2, NULL, sha256(convert_to(body_v2, 'UTF8')))
    RETURNING id INTO v2;

    INSERT INTO free_chapter_storage_objects
        (chapter_id, version_id, bucket, object_key, sha256, size_bytes)
    VALUES (
        fixture_chapter, v2, 'tor-fnd01-private',
        'private/free-chapters/' || fixture_chapter || '/' || v2 || '.md',
        sha256(convert_to(body_v2, 'UTF8')),
        octet_length(convert_to(body_v2, 'UTF8'))
    );

    INSERT INTO chapter_version_content_shapes
        (chapter_id, version_id, content_sha256,
         block_utf16_lengths, last_readable_block_index, last_readable_char_offset)
    VALUES (
        fixture_chapter, v2, sha256(convert_to(body_v2, 'UTF8')),
        ARRAY[char_length(body_v2)], 1, char_length(body_v2)
    );

    UPDATE chapter_publications
       SET version_id = v2, published_at = clock_timestamp()
     WHERE chapter_id = fixture_chapter;
END
$fnd01_retained_seed$;

-- An independent snapshot taken BEFORE the subsequent SQL migrations.
-- Every checked field must remain identical after the upgrade.
CREATE TABLE fnd01_retained_expected AS
SELECT b.id AS book_id, c.id AS chapter_id, v.id AS version_id,
       v.version_no, v.content_sha256 AS body_sha256,
       f.bucket, f.object_key, f.sha256 AS object_sha256,
       f.size_bytes, s.block_utf16_lengths,
       s.last_readable_block_index, s.last_readable_char_offset,
       p.version_id = v.id AS is_current,
       EXISTS (
         SELECT 1 FROM reader_saved_free_chapters pin
         WHERE pin.chapter_id = c.id AND pin.version_id = v.id
       ) AS is_saved
FROM books b
JOIN chapters c ON c.book_id = b.id
JOIN chapter_versions v ON v.chapter_id = c.id
JOIN free_chapter_storage_objects f
  ON f.chapter_id = c.id AND f.version_id = v.id
JOIN chapter_version_content_shapes s
  ON s.chapter_id = c.id AND s.version_id = v.id
JOIN chapter_publications p ON p.chapter_id = c.id
WHERE b.source_identity = 'fnd01-retained-s3-book';

DO $fnd01_seed_assert$
BEGIN
    IF (SELECT count(*) FROM fnd01_retained_expected) <> 2
       OR (SELECT count(*) FROM fnd01_retained_expected WHERE is_current) <> 1
       OR (SELECT count(*) FROM fnd01_retained_expected WHERE is_saved) <> 1
       OR EXISTS (
          SELECT 1 FROM fnd01_retained_expected
          WHERE body_sha256 <> object_sha256 OR size_bytes <= 0
       )
       OR EXISTS (
          SELECT 1 FROM chapter_versions v
          JOIN fnd01_retained_expected e ON e.version_id = v.id
          WHERE v.body_markdown IS NOT NULL
       ) THEN
        RAISE EXCEPTION 'FND01 seed did not produce two distinct S3-only editions';
    END IF;
END
$fnd01_seed_assert$;
