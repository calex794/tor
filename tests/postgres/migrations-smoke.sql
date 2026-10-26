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
    IF actual_count < 21 THEN
        RAISE EXCEPTION 'Expected at least 21 domain tables, got %', actual_count;
    END IF;
    IF to_regclass('public.cover_variant_jobs') IS NULL
       OR to_regclass('public.cover_variants') IS NULL
       OR to_regclass('public.book_cover_slots') IS NULL
       OR to_regclass('public.cover_upload_objects') IS NULL
       OR to_regclass('public.reader_book_positions') IS NULL
       OR to_regclass('public.chapter_purchase_facts') IS NULL
       OR to_regclass('public.chapter_purchase_commands') IS NULL THEN
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
