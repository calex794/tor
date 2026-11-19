-- FND-05 INTERNAL: a real already purchased PAID V1, a newer published
-- V2, and the buyer's historical reading position. Disposable tor_ci only.
BEGIN;
SELECT id AS book_id
FROM books WHERE source_identity='ci:i11:backup-public'
  AND target_language='ru'
\gset

INSERT INTO users(email_normalized, display_name, password_hash)
VALUES ('i11-restored-buyer@example.invalid', 'I11 retained paid buyer', '$argon2id$ci-fixture')
RETURNING id AS buyer_id
\gset

INSERT INTO chapters(book_id, chapter_no, title, visibility, access_level, price_amount)
VALUES (:book_id, 1, 'I11 historical paid chapter', 'public', 'paid', 157)
RETURNING id AS chapter_id
\gset

INSERT INTO chapter_versions(chapter_id, version_no, body_markdown, content_sha256)
VALUES (:chapter_id, 1, 'I11 purchased paid version V1',
        sha256('I11 purchased paid version V1'::bytea))
RETURNING id AS v1_id
\gset

INSERT INTO chapter_publications(chapter_id, version_id)
VALUES (:chapter_id, :v1_id);

INSERT INTO chapter_purchase_facts(user_id, chapter_id, version_id,
                                   settlement_reference, amount_minor, currency)
VALUES (:buyer_id, :chapter_id, :v1_id, 'ci:i11:paid-v1-purchase', 157, 'RUB')
RETURNING id AS purchase_id
\gset

INSERT INTO reader_purchased_positions(user_id, purchase_id, chapter_id,
                                       version_id, block_index, char_offset)
VALUES (:buyer_id, :purchase_id, :chapter_id, :v1_id, 2, 3);

INSERT INTO chapter_versions(chapter_id, version_no, body_markdown, content_sha256)
VALUES (:chapter_id, 2, 'I11 current paid version V2',
        sha256('I11 current paid version V2'::bytea))
RETURNING id AS v2_id
\gset

UPDATE chapter_publications SET version_id=:v2_id WHERE chapter_id=:chapter_id;
COMMIT;
