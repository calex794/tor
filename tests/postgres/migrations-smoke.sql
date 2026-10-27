-- Executed only against disposable CI PostgreSQL 18 test databases.
-- Exercise real constraints, version immutability, and draft filtering.
DO $tor_migrations_smoke$
DECLARE
    actual_count integer;
    new_user_id bigint;
    public_book_id bigint;
    new_chapter_id bigint;
    version_id bigint;
BEGIN
    SELECT count(*) INTO actual_count
      FROM pg_catalog.pg_tables WHERE schemaname='public';
    IF actual_count < 23 THEN
        RAISE EXCEPTION 'Expected at least 21 domain tables, got %', actual_count;
    END IF;
    IF to_regclass('public.cover_variant_jobs') IS NULL
       OR to_regclass('public.cover_variants') IS NULL
       OR to_regclass('public.book_cover_slots') IS NULL
       OR to_regclass('public.ril_flush_batches') IS NULL
       OR to_regclass('public.ril_flush_routes') IS NULL
       OR to_regclass('public.ril_flush_transitions') IS NULL
       OR to_regclass('public.ril_route_hourly') IS NULL
       OR to_regclass('public.ril_transition_hourly') IS NULL
       OR to_regclass('public.ril_daily_rollup') IS NULL
       OR to_regclass('public.ril_flush_sql_costs') IS NULL
       OR to_regclass('public.ril_sql_cost_hourly') IS NULL
       OR to_regclass('public.cover_upload_objects') IS NULL
       OR to_regclass('public.reader_book_positions') IS NULL
       OR to_regclass('public.chapter_purchase_facts') IS NULL
       OR to_regclass('public.chapter_purchase_commands') IS NULL
       OR to_regclass('public.chapter_encrypted_release_events') IS NULL THEN
        RAISE EXCEPTION 'Missing contract tables';
    END IF;

    INSERT INTO users(email_normalized,display_name,password_hash)
      VALUES('migration-test@example.invalid','Migration Fixture','$argon2id$ci-placeholder')
      RETURNING id INTO new_user_id;

    INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
      VALUES ('ci-public-book','ru','CI Public Book','public',new_user_id)
      RETURNING id INTO public_book_id;

    INSERT INTO books(source_identity,target_language,title,visibility)
      VALUES ('ci-hidden-book','ru','Hidden Draft','draft');

    SELECT count(*) INTO actual_count FROM books
      WHERE source_identity IN ('ci-public-book', 'ci-hidden-book')
        AND visibility='public';
    IF actual_count <> 1 OR EXISTS (
        SELECT 1 FROM books WHERE source_identity='ci-hidden-book' AND visibility='public'
    ) THEN
        RAISE EXCEPTION 'Draft selection wrong; public count=%', actual_count;
    END IF;

    BEGIN
      INSERT INTO books(source_identity,target_language,title,visibility)
        VALUES ('ci-public-book','ru','Duplicate','public');
      RAISE EXCEPTION 'Canonical Book uniqueness not enforced';
    EXCEPTION WHEN unique_violation THEN
      NULL;
    END;

    INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
      VALUES (public_book_id,1,'CI Chapter','public','free',0)
      RETURNING id INTO new_chapter_id;
    INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
      VALUES (new_chapter_id,1,'# First chapter')
      RETURNING id INTO version_id;
    INSERT INTO chapter_publications(chapter_id,version_id)
      VALUES (new_chapter_id,version_id);

    BEGIN
      UPDATE chapter_versions SET body_markdown='changed' WHERE id=version_id;
      RAISE EXCEPTION 'Immutable version update was allowed';
    EXCEPTION WHEN check_violation THEN
      NULL;
    END;

    BEGIN
      INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
        VALUES (public_book_id,2,'Invalid paid free price','public','paid',0);
      RAISE EXCEPTION 'Invalid paid price was accepted';
    EXCEPTION WHEN check_violation THEN
      NULL;
    END;

    IF NOT EXISTS(
      SELECT 1 FROM chapters c
        JOIN chapter_publications p ON p.chapter_id=c.id
        JOIN chapter_versions v ON v.id=p.version_id
       WHERE c.id=new_chapter_id AND v.body_markdown='# First chapter'
    ) THEN
      RAISE EXCEPTION 'Publication pointer inconsistent';
    END IF;
END
$tor_migrations_smoke$;


-- Shadow opt-in must remain disabled for old books and must not expose chapter
-- bodies or unpublished content. This is run on fresh and upgraded databases.
DO $shadow_smoke$
DECLARE
    book_id bigint;
    chapter_id bigint;
    version_id bigint;
    visible_count integer;
BEGIN
    SELECT id INTO book_id FROM books WHERE source_identity='ci-public-book';
    IF (SELECT shadow_enabled FROM books WHERE id=book_id) <> FALSE
       OR (SELECT seo_index_owner FROM books WHERE id=book_id) <> 'pandaolate' THEN
        RAISE EXCEPTION 'Shadow default-off / SEO owner invariant failed';
    END IF;

    SELECT count(*) INTO visible_count FROM shadow_public_books WHERE id=book_id;
    IF visible_count <> 0 THEN
        RAISE EXCEPTION 'Shadow leaked a default-off book';
    END IF;

    BEGIN
        UPDATE books SET seo_index_owner='shadow' WHERE id=book_id;
        RAISE EXCEPTION 'Shadow SEO owner accepted without opt-in';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    UPDATE books SET shadow_enabled=TRUE, seo_index_owner='shadow'
      WHERE id=book_id;
    IF (SELECT count(*) FROM shadow_public_books WHERE id=book_id) <> 1 THEN
        RAISE EXCEPTION 'Shadow manually enabled book is missing';
    END IF;
    IF (SELECT count(*) FROM shadow_public_books WHERE source_identity='ci-hidden-book') <> 0 THEN
        RAISE EXCEPTION 'Shadow leaked a draft';
    END IF;

    INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
      VALUES(book_id,3,'Paid metadata allowed','public','paid',150)
      RETURNING id INTO chapter_id;
    INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
      VALUES(chapter_id,1,'SECRET-PAID-MARKER')
      RETURNING id INTO version_id;
    INSERT INTO chapter_publications(chapter_id,version_id)
      VALUES(chapter_id,version_id);
    IF (SELECT count(*) FROM shadow_public_chapters WHERE id=chapter_id) <> 1 THEN
        RAISE EXCEPTION 'Paid metadata incorrectly vetoed manual Shadow opt-in';
    END IF;
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name='shadow_public_chapters'
          AND column_name IN ('body_markdown','price_amount','access_level')
    ) THEN
        RAISE EXCEPTION 'Shadow chapter view exposes restricted data';
    END IF;

    UPDATE books SET shadow_enabled=FALSE,seo_index_owner='pandaolate'
      WHERE id=book_id;
    IF EXISTS(SELECT 1 FROM shadow_public_books WHERE id=book_id)
       OR EXISTS(SELECT 1 FROM shadow_public_chapters WHERE book_id=book_id) THEN
        RAISE EXCEPTION 'Shadow manual withdrawal leaked a book or chapters';
    END IF;
END
$shadow_smoke$;
