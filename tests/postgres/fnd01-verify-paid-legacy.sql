-- FND-01/06: installing paid bodyless SQL must never rewrite historic paid
-- V1, move a purchased library pointer, or accept an unverified S3-only V3.
-- The fixture was committed before the additive bodyless migration.
DO $fnd01_paid_upgrade_verify$
DECLARE
    fixture_chapter_id BIGINT;
    fixture_buyer_id BIGINT;
    v1 BIGINT;
    v3 BIGINT;
BEGIN
    SELECT chapter_id INTO fixture_chapter_id
    FROM fnd01_paid_expected LIMIT 1;
    SELECT v.id INTO v1
    FROM chapter_versions v
    WHERE v.chapter_id = fixture_chapter_id AND v.version_no = 1;
    SELECT pf.user_id INTO fixture_buyer_id
    FROM chapter_purchase_facts pf
    WHERE pf.chapter_id = fixture_chapter_id AND pf.version_id = v1
      AND pf.settlement_reference = 'fnd01-retained-paid-v1';

    IF fixture_chapter_id IS NULL OR v1 IS NULL OR fixture_buyer_id IS NULL
       OR (SELECT count(*) FROM fnd01_paid_expected) <> 2
       OR (SELECT count(*) FROM chapter_versions WHERE chapter_id = fixture_chapter_id) <> 2
       OR NOT EXISTS (
           SELECT 1 FROM chapter_purchase_facts pf
           JOIN library_purchased_chapters l
             ON l.purchase_id = pf.id
            AND l.user_id = pf.user_id
            AND l.chapter_id = pf.chapter_id
            AND l.active_version_id = pf.version_id
           WHERE pf.chapter_id = fixture_chapter_id AND pf.version_id = v1
             AND pf.user_id = fixture_buyer_id
             AND pf.settlement_reference = 'fnd01-retained-paid-v1'
             AND pf.amount_minor = 150 AND pf.currency = 'RUB'
       )
       OR (SELECT count(*) FROM chapter_purchase_facts
           WHERE chapter_id = fixture_chapter_id) <> 1
    THEN
        RAISE EXCEPTION 'FND01 historical paid V1 purchase or library right lost';
    END IF;

    -- Set-equality in both directions detects any altered VERSION/PG body,
    -- current pointer, SHA, or historical purchase identity after migration.
    IF EXISTS (
        SELECT book_id, chapter_id, version_id, version_no,
               body_markdown, content_sha256, is_current, is_purchased
        FROM fnd01_paid_expected
        EXCEPT
        SELECT b.id, c.id, v.id, v.version_no, v.body_markdown, v.content_sha256,
               p.version_id = v.id,
               EXISTS (
                   SELECT 1 FROM chapter_purchase_facts pf
                   JOIN library_purchased_chapters l
                     ON l.purchase_id = pf.id
                    AND l.user_id = pf.user_id
                    AND l.chapter_id = pf.chapter_id
                    AND l.active_version_id = pf.version_id
                   WHERE pf.chapter_id = c.id AND pf.version_id = v.id
                     AND pf.settlement_reference = 'fnd01-retained-paid-v1'
               )
        FROM books b
        JOIN chapters c ON c.book_id = b.id
        JOIN chapter_versions v ON v.chapter_id = c.id
        JOIN chapter_publications p ON p.chapter_id = c.id
        WHERE c.id = fixture_chapter_id
    ) OR EXISTS (
        SELECT b.id, c.id, v.id, v.version_no, v.body_markdown, v.content_sha256,
               p.version_id = v.id,
               EXISTS (
                   SELECT 1 FROM chapter_purchase_facts pf
                   JOIN library_purchased_chapters l
                     ON l.purchase_id = pf.id
                    AND l.user_id = pf.user_id
                    AND l.chapter_id = pf.chapter_id
                    AND l.active_version_id = pf.version_id
                   WHERE pf.chapter_id = c.id AND pf.version_id = v.id
                     AND pf.settlement_reference = 'fnd01-retained-paid-v1'
               )
        FROM books b
        JOIN chapters c ON c.book_id = b.id
        JOIN chapter_versions v ON v.chapter_id = c.id
        JOIN chapter_publications p ON p.chapter_id = c.id
        WHERE c.id = fixture_chapter_id
        EXCEPT
        SELECT book_id, chapter_id, version_id, version_no,
               body_markdown, content_sha256, is_current, is_purchased
        FROM fnd01_paid_expected
    ) THEN
        RAISE EXCEPTION 'FND01 paid V1/V2, digest, current pointer or purchase changed';
    END IF;

    BEGIN
        UPDATE chapter_versions SET body_markdown = '# forbidden rewrite'
        WHERE id = v1 AND chapter_id = fixture_chapter_id;
        RAISE EXCEPTION 'FND01 accepted a rewrite of purchased historical paid V1';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    BEGIN
        INSERT INTO chapter_versions(chapter_id, version_no, body_markdown, content_sha256)
        VALUES (
            fixture_chapter_id, 3, NULL,
            sha256(convert_to('# unverified paid V3', 'UTF8'))
        ) RETURNING id INTO v3;
        -- The new trigger requires a verified G object and matching manifest.
        UPDATE chapter_publications
           SET version_id = v3
         WHERE chapter_id = fixture_chapter_id;
        RAISE EXCEPTION 'FND01 allowed unverified paid bodyless publication';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    IF (SELECT count(*) FROM chapter_versions WHERE chapter_id = fixture_chapter_id) <> 2
       OR (SELECT count(*) FROM library_purchased_chapters
           WHERE chapter_id = fixture_chapter_id
             AND user_id = fixture_buyer_id
             AND active_version_id = v1) <> 1
    THEN
        RAISE EXCEPTION 'FND01 failed to roll back attempted unverified V3';
    END IF;

    RAISE NOTICE 'FND01_RETAINED_PAID_V1_UPGRADE_OK: legacy paid V1 buyer, V2 current and new bodyless guard preserved';
END
$fnd01_paid_upgrade_verify$;
