-- FND-01: verify an already-retained S3-only V1/V2 after real SQL upgrades.
-- This validates PostgreSQL identities/rights/receipts, not S3 object bytes.
DO $fnd01_retained_verify$
DECLARE
    retained_book BIGINT;
    retained_chapter BIGINT;
BEGIN
    SELECT book_id, chapter_id INTO retained_book, retained_chapter
    FROM fnd01_retained_expected LIMIT 1;

    IF retained_book IS NULL
       OR (SELECT count(*) FROM fnd01_retained_expected) <> 2
       OR (SELECT count(*) FROM chapter_versions
           WHERE chapter_id = retained_chapter) <> 2
       OR NOT EXISTS (
           SELECT 1 FROM books b
           JOIN chapters c ON c.book_id = b.id
           WHERE b.id = retained_book AND c.id = retained_chapter
             AND b.source_identity = 'fnd01-retained-s3-book'
             AND c.access_level = 'free' AND c.price_amount = 0
       ) THEN
        RAISE EXCEPTION 'FND01 retained Book/Chapter identity lost during upgrade';
    END IF;

    IF EXISTS (
        SELECT book_id, chapter_id, version_id, version_no,
               body_sha256, bucket, object_key, object_sha256,
               size_bytes, block_utf16_lengths,
               last_readable_block_index, last_readable_char_offset,
               is_current, is_saved
        FROM fnd01_retained_expected
        EXCEPT
        SELECT b.id, c.id, v.id, v.version_no,
               v.content_sha256, f.bucket, f.object_key, f.sha256,
               f.size_bytes, s.block_utf16_lengths,
               s.last_readable_block_index, s.last_readable_char_offset,
               p.version_id = v.id,
               EXISTS (
                   SELECT 1 FROM reader_saved_free_chapters pin
                   WHERE pin.chapter_id = c.id AND pin.version_id = v.id
               )
        FROM books b
        JOIN chapters c ON c.book_id = b.id
        JOIN chapter_versions v ON v.chapter_id = c.id
        JOIN free_chapter_storage_objects f
             ON f.chapter_id = c.id AND f.version_id = v.id
        JOIN chapter_version_content_shapes s
             ON s.chapter_id = c.id AND s.version_id = v.id
        JOIN chapter_publications p ON p.chapter_id = c.id
        WHERE b.id = retained_book AND c.id = retained_chapter
    ) OR EXISTS (
        SELECT b.id, c.id, v.id, v.version_no,
               v.content_sha256, f.bucket, f.object_key, f.sha256,
               f.size_bytes, s.block_utf16_lengths,
               s.last_readable_block_index, s.last_readable_char_offset,
               p.version_id = v.id,
               EXISTS (
                   SELECT 1 FROM reader_saved_free_chapters pin
                   WHERE pin.chapter_id = c.id AND pin.version_id = v.id
               )
        FROM books b
        JOIN chapters c ON c.book_id = b.id
        JOIN chapter_versions v ON v.chapter_id = c.id
        JOIN free_chapter_storage_objects f
             ON f.chapter_id = c.id AND f.version_id = v.id
        JOIN chapter_version_content_shapes s
             ON s.chapter_id = c.id AND s.version_id = v.id
        JOIN chapter_publications p ON p.chapter_id = c.id
        WHERE b.id = retained_book AND c.id = retained_chapter
        EXCEPT
        SELECT book_id, chapter_id, version_id, version_no,
               body_sha256, bucket, object_key, object_sha256,
               size_bytes, block_utf16_lengths,
               last_readable_block_index, last_readable_char_offset,
               is_current, is_saved
        FROM fnd01_retained_expected
    ) THEN
        RAISE EXCEPTION 'FND01 retained V1/V2/S3 receipt/pin changed after upgrade';
    END IF;

    IF EXISTS (
        SELECT 1 FROM chapter_versions v
        JOIN fnd01_retained_expected e ON e.version_id = v.id
        WHERE v.body_markdown IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'FND01 upgrade unexpectedly restored a PostgreSQL chapter body';
    END IF;

    -- Negative controls: the upgrade must not remove immutability guards.
    BEGIN
        UPDATE chapter_versions
           SET content_sha256 = decode(repeat('ab', 32), 'hex')
         WHERE chapter_id = retained_chapter;
        RAISE EXCEPTION 'FND01 accepted an immutable edition rewrite';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    BEGIN
        UPDATE free_chapter_storage_objects
           SET object_key = object_key || '.altered'
         WHERE chapter_id = retained_chapter;
        RAISE EXCEPTION 'FND01 accepted an immutable storage reference rewrite';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    RAISE NOTICE 'FND01_RETAINED_S3_UPGRADE_OK: Book, V1/V2, S3 receipts and saved FREE V1 survive';
END
$fnd01_retained_verify$;
