-- DROP-08b: synthetic PG18 fixture, all tables are session-local shadows.
-- No writes to the migrated public schema or real book/user records.
-- Invoke only through the guarded disposable CI harness.
\set ON_ERROR_STOP on
CREATE TEMP TABLE book_layer_free_archive_versions (
    book_id bigint NOT NULL, chapter_id bigint NOT NULL, version_id bigint NOT NULL
);
CREATE TEMP TABLE reader_saved_free_chapters (
    chapter_id bigint NOT NULL, version_id bigint NOT NULL
);
CREATE TEMP TABLE chapters (
    id bigint NOT NULL, book_id bigint NOT NULL,
    access_level text NOT NULL, price_amount bigint NOT NULL
);
CREATE TEMP TABLE chapter_versions (
    chapter_id bigint NOT NULL, id bigint NOT NULL,
    body_markdown text, content_sha256 bytea
);
CREATE TEMP TABLE free_chapter_storage_objects (
    chapter_id bigint NOT NULL, version_id bigint NOT NULL, sha256 bytea
);
CREATE TEMP TABLE free_edition_active_storage (
    chapter_id bigint NOT NULL, version_id bigint NOT NULL, generation bigint NOT NULL
);
CREATE TEMP TABLE free_edition_storage_generations (
    chapter_id bigint NOT NULL, version_id bigint NOT NULL,
    generation bigint NOT NULL, placement_kind text NOT NULL,
    content_sha256 bytea NOT NULL, pack_id bigint
);
CREATE TEMP TABLE free_storage_packs (id bigint NOT NULL);

INSERT INTO chapters VALUES
    (101,11,CASE WHEN :'scenario'='paid_layer_chapter' THEN 'paid' ELSE 'free' END,0),
    (202,22,'free',0);
INSERT INTO chapter_versions VALUES
    (101,1001,NULL,decode(repeat('11',32),'hex')),
    (202,2002,NULL,decode(repeat('22',32),'hex'));
INSERT INTO book_layer_free_archive_versions VALUES
    ((CASE WHEN :'scenario'='wrong_layer_book' THEN 22 ELSE 11 END),101,1001);
INSERT INTO reader_saved_free_chapters VALUES (202,2002);

INSERT INTO free_storage_packs VALUES (7);
INSERT INTO free_edition_storage_generations VALUES
    (101,1001,1,'pack',
     (CASE WHEN :'scenario'='wrong_pack_sha'
           THEN decode(repeat('33',32),'hex')
           ELSE decode(repeat('11',32),'hex') END),7);
INSERT INTO free_edition_active_storage (chapter_id,version_id,generation)
    SELECT 101,1001,1 WHERE :'scenario' <> 'candidate_only';
INSERT INTO free_chapter_storage_objects (chapter_id,version_id,sha256)
    VALUES (202,2002,
      CASE WHEN :'scenario'='wrong_single_sha'
           THEN decode(repeat('33',32),'hex')
           ELSE decode(repeat('22',32),'hex') END);

-- Run exact production read-only export against session-local temp tables.
\i tools/drop08_export_historical_free_pins.sql
