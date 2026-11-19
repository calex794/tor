-- FND-01/06: seed a historical PG-backed PAID V1, an immutable purchase,
-- and an independent V2 BEFORE the new bodyless attachment SQL guard arrives.
-- Disposable PostgreSQL 18 only. No real checkout, S3, or payment.
DO $fnd01_paid_seed$
DECLARE
    owner_id BIGINT;
    buyer_id BIGINT;
    book_id BIGINT;
    fixture_chapter_id BIGINT;
    v1 BIGINT;
    v2 BIGINT;
    fixture_purchase_id BIGINT;
    old_body TEXT := '# Purchased paid V1 - retain unchanged';
    new_body TEXT := '# Newer paid V2 - do not replace V1';
BEGIN
    INSERT INTO users(email_normalized, display_name, password_hash)
    VALUES ('fnd01-paid-owner@example.invalid', 'FND01 Paid Owner', '$argon2id$ci-only')
    RETURNING id INTO owner_id;
    INSERT INTO users(email_normalized, display_name, password_hash)
    VALUES ('fnd01-paid-buyer@example.invalid', 'FND01 Paid Buyer', '$argon2id$ci-only')
    RETURNING id INTO buyer_id;
    INSERT INTO books(source_identity, target_language, title, visibility, owner_user_id, access_level)
    VALUES ('fnd01-retained-paid-book', 'ru', 'FND01 Purchased V1', 'public', owner_id, 'basic')
    RETURNING id INTO book_id;
    INSERT INTO chapters(book_id, chapter_no, title, visibility, access_level, price_amount)
    VALUES (book_id, 1, 'Historical paid chapter', 'public', 'paid', 150)
    RETURNING id INTO fixture_chapter_id;
    INSERT INTO chapter_versions(chapter_id, version_no, body_markdown, content_sha256)
    VALUES (fixture_chapter_id, 1, old_body, sha256(convert_to(old_body, 'UTF8')))
    RETURNING id INTO v1;
    INSERT INTO chapter_publications(chapter_id, version_id)
    VALUES (fixture_chapter_id, v1);

    -- The trusted historical purchase fact must pin exact V1, never CURRENT.
    INSERT INTO chapter_purchase_facts
        (user_id, chapter_id, version_id, settlement_reference, amount_minor, currency)
    VALUES (buyer_id, fixture_chapter_id, v1, 'fnd01-retained-paid-v1', 150, 'RUB')
    RETURNING id INTO fixture_purchase_id;
    IF NOT EXISTS (
        SELECT 1 FROM library_purchased_chapters
        WHERE user_id = buyer_id AND chapter_id = fixture_chapter_id
          AND active_version_id = v1 AND purchase_id = fixture_purchase_id
    ) THEN
        RAISE EXCEPTION 'FND01 paid seed did not create the V1 library pointer';
    END IF;

    INSERT INTO chapter_versions(chapter_id, version_no, body_markdown, content_sha256)
    VALUES (fixture_chapter_id, 2, new_body, sha256(convert_to(new_body, 'UTF8')))
    RETURNING id INTO v2;
    UPDATE chapter_publications
       SET version_id = v2, published_at = clock_timestamp()
     WHERE chapter_publications.chapter_id = fixture_chapter_id;
END
$fnd01_paid_seed$;

-- Snapshot is made before the migration. The post-upgrade assertion compares
-- both directions to detect even silent metadata or grant changes.
CREATE TABLE fnd01_paid_expected AS
SELECT b.id AS book_id, c.id AS chapter_id,
       v.id AS version_id, v.version_no,
       v.body_markdown, v.content_sha256,
       p.version_id = v.id AS is_current,
       EXISTS (
           SELECT 1 FROM chapter_purchase_facts pf
           JOIN library_purchased_chapters l
             ON l.purchase_id = pf.id
            AND l.user_id = pf.user_id
            AND l.chapter_id = pf.chapter_id
            AND l.active_version_id = pf.version_id
           WHERE pf.chapter_id = c.id AND pf.version_id = v.id
             AND pf.settlement_reference = 'fnd01-retained-paid-v1'
       ) AS is_purchased
FROM books b
JOIN chapters c ON c.book_id = b.id
JOIN chapter_versions v ON v.chapter_id = c.id
JOIN chapter_publications p ON p.chapter_id = c.id
WHERE b.source_identity = 'fnd01-retained-paid-book';

DO $fnd01_paid_seed_assert$
BEGIN
    IF (SELECT count(*) FROM fnd01_paid_expected) <> 2
       OR (SELECT count(*) FROM fnd01_paid_expected WHERE is_current) <> 1
       OR (SELECT count(*) FROM fnd01_paid_expected WHERE is_purchased) <> 1
       OR NOT EXISTS (
           SELECT 1 FROM fnd01_paid_expected
           WHERE version_no = 1 AND is_purchased AND NOT is_current
       )
       OR EXISTS (
           SELECT 1 FROM fnd01_paid_expected
           WHERE body_markdown IS NULL
              OR content_sha256 <> sha256(convert_to(body_markdown, 'UTF8'))
       ) THEN
        RAISE EXCEPTION 'FND01 paid fixture must retain purchased V1 and current V2';
    END IF;
END
$fnd01_paid_seed_assert$;
