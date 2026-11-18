BEGIN;
SELECT id AS book_id FROM books WHERE source_identity='ci:s061:public' \gset
INSERT INTO users(email_normalized, display_name, password_hash)
VALUES ('s061-catalog@example.invalid', 'S061 installed reader', '$argon2id$fixture')
RETURNING id AS reader_id \gset

INSERT INTO chapters(book_id, chapter_no, title, visibility, access_level)
VALUES (:book_id, 1, 'S061 persistent FREE chapter', 'public', 'free')
RETURNING id AS free_id \gset
INSERT INTO chapter_versions(chapter_id, version_no, body_markdown, content_sha256)
VALUES (:free_id, 1, NULL, sha256('S061 historical free edition V1'::bytea))
RETURNING id AS saved_id \gset
INSERT INTO free_chapter_storage_objects(chapter_id, version_id, bucket, object_key, sha256, size_bytes)
SELECT chapter_id, id, 'tor-s061-catalog-ci',
       'private/free-chapters/' || chapter_id || '/' || id || '.md', content_sha256,
       octet_length('S061 historical free edition V1'::text)
FROM chapter_versions WHERE id=:saved_id;
INSERT INTO chapter_publications(chapter_id, version_id) VALUES (:free_id, :saved_id);
INSERT INTO reader_saved_free_chapters(user_id, chapter_id, version_id)
VALUES (:reader_id, :free_id, :saved_id);
INSERT INTO book_layer_free_archive_versions(book_id, request_revision, chapter_id, version_id)
VALUES (:book_id, 1, :free_id, :saved_id);
INSERT INTO reader_book_positions(user_id, book_id, chapter_id, chapter_no, chapter_revision,
                                  version_id, block_index, char_offset)
VALUES (:reader_id, :book_id, :free_id, 1, 0, :saved_id, 2, 4);
INSERT INTO chapter_versions(chapter_id, version_no, body_markdown, content_sha256)
VALUES (:free_id, 2, 'S061 current free edition V2', sha256('S061 current free edition V2'::bytea))
RETURNING id AS current_id \gset
UPDATE chapter_publications SET version_id=:current_id WHERE chapter_id=:free_id;

INSERT INTO chapters(book_id, chapter_no, title, visibility, access_level, price_amount)
VALUES (:book_id, 2, 'S061 persistent paid chapter', 'public', 'paid', 157)
RETURNING id AS paid_id \gset
INSERT INTO chapter_versions(chapter_id, version_no, body_markdown, content_sha256)
VALUES (:paid_id, 1, 'S061 paid fixture', sha256('S061 paid fixture'::bytea))
RETURNING id AS paid_version \gset
INSERT INTO chapter_publications(chapter_id, version_id) VALUES (:paid_id, :paid_version);
INSERT INTO chapter_purchase_facts(user_id, chapter_id, version_id, settlement_reference,
                                   amount_minor, currency)
VALUES (:reader_id, :paid_id, :paid_version, 'ci:s061:catalog:purchase', 157, 'RUB')
RETURNING id AS purchase_id \gset
INSERT INTO reader_purchased_positions(user_id, purchase_id, chapter_id, version_id,
                                       block_index, char_offset)
VALUES (:reader_id, :purchase_id, :paid_id, :paid_version, 2, 3);
SELECT :free_id, :paid_id, :saved_id, :current_id;
COMMIT;
