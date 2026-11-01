-- B/TAX-03: rollback-only curator smoke on disposable tor_ci database.
-- This file is invoked only as tor_migrator from PostgreSQL CI.
BEGIN;
DO $$
DECLARE
    suffix TEXT := substr(md5(clock_timestamp()::text),1,10);
    label TEXT;
    alias_name TEXT;
    v_term_id BIGINT;
    second_id BIGINT;
    book_id BIGINT;
    actual_tags TEXT[];
    actual_links BIGINT[];
    actual_revision BIGINT;
    rejected BOOLEAN;
BEGIN
    IF current_database()<>'tor_ci' OR current_user<>'tor_migrator' THEN
        RAISE EXCEPTION 'only disposable migration-owner tor_ci is accepted';
    END IF;
    label := 'CurationTest' || suffix;
    alias_name := 'LegacyAlias' || suffix;
    v_term_id := admin_taxonomy_add_term('tag',label,'ci-smoke');
    IF v_term_id<=0 THEN RAISE EXCEPTION 'new term not created'; END IF;
    PERFORM admin_taxonomy_add_alias('tag',v_term_id,alias_name,'ci-smoke');
    PERFORM admin_taxonomy_add_alias('tag',term_id,alias_name,'ci-smoke');
    -- Qualified term-ID check; an idempotent alias must have only one row.
    IF (SELECT count(*) FROM book_taxonomy_aliases
        WHERE kind='tag' AND search_key=admin_taxonomy_key(alias_name)
          AND book_taxonomy_aliases.term_id=v_term_id) <> 1 THEN
        RAISE EXCEPTION 'alias idempotency or normalization failed';
    END IF;
    rejected:=false;
    second_id := admin_taxonomy_add_term('tag','Other'||suffix,'ci-smoke');
    BEGIN
        PERFORM admin_taxonomy_add_alias('tag',second_id,alias_name,'ci-smoke');
    EXCEPTION WHEN unique_violation THEN rejected:=true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'cross-term alias collision accepted'; END IF;
    rejected:=false;
    BEGIN
        PERFORM admin_taxonomy_add_term('tag',alias_name,'ci-smoke');
    EXCEPTION WHEN unique_violation THEN rejected:=true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'canonical/alias collision accepted'; END IF;
    INSERT INTO books(source_identity,target_language,title,visibility,tags)
        VALUES('ci-curation:'||suffix,'ru','Curation isolated book','draft',
               ARRAY[alias_name]) RETURNING id INTO book_id;
    IF NOT admin_taxonomy_reconcile_book(book_id,'ci-smoke') THEN
        RAISE EXCEPTION 'expected canonical reconciliation';
    END IF;
    SELECT tags,taxonomy_revision INTO actual_tags,actual_revision
      FROM books WHERE id=book_id;
    SELECT array_agg(term_id ORDER BY position) INTO actual_links
      FROM book_taxonomy_links WHERE book_taxonomy_links.book_id=book_id;
    IF actual_tags<>ARRAY[admin_taxonomy_key(label)]
       OR actual_links<>ARRAY[v_term_id] OR actual_revision<>2 THEN
        RAISE EXCEPTION 'book legacy arrays, links or revision mismatch';
    END IF;
    IF admin_taxonomy_reconcile_book(book_id,'ci-smoke') THEN
        RAISE EXCEPTION 'idempotent reconciliation rewrote book';
    END IF;
    UPDATE books SET tags=ARRAY['unknown'||suffix] WHERE id=book_id;
    rejected:=false;
    BEGIN
        PERFORM admin_taxonomy_reconcile_book(book_id,'ci-smoke');
    EXCEPTION WHEN invalid_parameter_value THEN rejected:=true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'unknown legacy label not rejected'; END IF;
    SELECT tags INTO actual_tags FROM books WHERE id=book_id;
    IF actual_tags<>ARRAY['unknown'||suffix] THEN
        RAISE EXCEPTION 'unknown legacy metadata was discarded';
    END IF;
    IF (SELECT count(*) FROM book_taxonomy_curation_events
        WHERE book_taxonomy_curation_events.book_id=book_id
          AND action='reconcile_book')<>1 THEN
        RAISE EXCEPTION 'unexpected number of audited reconciliations';
    END IF;
END
$$;
ROLLBACK;
