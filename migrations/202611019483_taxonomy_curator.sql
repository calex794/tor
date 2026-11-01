-- B/TAX-03. Curation is explicitly a migrator-only, offline operation.
-- TOR has no reviewed staff-role authorization model. Never grant curation
-- to tor_app, a site session, or the read-only operator diagnostics endpoint.
CREATE TABLE book_taxonomy_curation_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    action TEXT NOT NULL CHECK (action IN ('create_term','add_alias','reconcile_book')),
    kind TEXT CHECK (kind IN ('genre','tag','fandom')),
    term_id BIGINT REFERENCES book_taxonomy_terms(id),
    book_id BIGINT REFERENCES books(id) ON DELETE SET NULL,
    curator TEXT NOT NULL CHECK (curator ~ '^[a-zA-Z0-9_.-]{3,80}$'),
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX book_taxonomy_curation_events_newest
    ON book_taxonomy_curation_events(created_at DESC,id DESC);

-- Share a single NFKC key with the authoring path.
CREATE FUNCTION admin_taxonomy_key(value TEXT) RETURNS TEXT
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $
    SELECT book_taxonomy_key(value);
$;

CREATE FUNCTION admin_taxonomy_guard(curator_tag TEXT) RETURNS void
LANGUAGE plpgsql SECURITY INVOKER AS $$
BEGIN
    IF current_user <> 'tor_migrator' THEN
        RAISE EXCEPTION 'taxonomy curation requires migration-owner role'
            USING ERRCODE='42501';
    END IF;
    IF curator_tag IS NULL
       OR curator_tag !~ '^[a-zA-Z0-9_.-]{3,80}$' THEN
        RAISE EXCEPTION 'invalid curator tag' USING ERRCODE='22023';
    END IF;
    -- Serialize curator operations across the canonical/alias namespaces.
    PERFORM pg_advisory_xact_lock(20261101,9483);
END;
$$;

CREATE FUNCTION admin_taxonomy_add_term(
    desired_kind TEXT, desired_label TEXT, curator_tag TEXT
) RETURNS BIGINT LANGUAGE plpgsql SECURITY INVOKER AS $$
DECLARE k TEXT; result_id BIGINT; previous_label TEXT; previous_status TEXT;
BEGIN
    PERFORM admin_taxonomy_guard(curator_tag);
    IF desired_kind NOT IN ('genre','tag','fandom') OR desired_kind IS NULL
       OR desired_label IS NULL OR desired_label <> btrim(desired_label)
       OR NOT char_length(desired_label) BETWEEN 1 AND 40
       OR desired_label ~ '[[:cntrl:]]' THEN
        RAISE EXCEPTION 'invalid canonical term' USING ERRCODE='22023';
    END IF;
    k := admin_taxonomy_key(desired_label);
    IF k = '' OR char_length(k) > 80 THEN
        RAISE EXCEPTION 'invalid normalized term key' USING ERRCODE='22023';
    END IF;
    IF EXISTS (SELECT 1 FROM book_taxonomy_aliases
               WHERE kind=desired_kind AND search_key=k) THEN
        RAISE EXCEPTION 'canonical name conflicts with an existing alias'
            USING ERRCODE='23505';
    END IF;
    -- A reviewed CSV may be replayed without duplicate IDs or audit events.
    -- Inactive terms and alternate spellings of the same key are conflicts.
    SELECT id,label,status INTO result_id,previous_label,previous_status
      FROM book_taxonomy_terms
     WHERE kind=desired_kind AND search_key=k;
    IF FOUND THEN
        IF previous_status='active' AND previous_label=desired_label THEN
            RETURN result_id;
        END IF;
        RAISE EXCEPTION 'canonical name collision or inactive term'
            USING ERRCODE='23505';
    END IF;
    INSERT INTO book_taxonomy_terms(kind,label,search_key)
    VALUES(desired_kind,desired_label,k) RETURNING id INTO result_id;
    INSERT INTO book_taxonomy_curation_events(action,kind,term_id,curator)
    VALUES('create_term',desired_kind,result_id,curator_tag);
    RETURN result_id;
END;
$$;

CREATE FUNCTION admin_taxonomy_add_alias(
    desired_kind TEXT, target_id BIGINT, raw_alias TEXT, curator_tag TEXT
) RETURNS void LANGUAGE plpgsql SECURITY INVOKER AS $$
DECLARE k TEXT; existing_id BIGINT; real_key TEXT;
BEGIN
    PERFORM admin_taxonomy_guard(curator_tag);
    IF desired_kind NOT IN ('genre','tag','fandom') OR desired_kind IS NULL
       OR raw_alias IS NULL OR raw_alias <> btrim(raw_alias)
       OR NOT char_length(raw_alias) BETWEEN 1 AND 80
       OR raw_alias ~ '[[:cntrl:]]' THEN
        RAISE EXCEPTION 'invalid taxonomy alias' USING ERRCODE='22023';
    END IF;
    k := admin_taxonomy_key(raw_alias);
    IF k='' OR char_length(k)>80 OR k ~ '[[:cntrl:]]' THEN
        RAISE EXCEPTION 'invalid normalized alias key' USING ERRCODE='22023';
    END IF;
    SELECT search_key INTO real_key FROM book_taxonomy_terms
    WHERE id=target_id AND kind=desired_kind AND status='active';
    IF real_key IS NULL THEN
        RAISE EXCEPTION 'alias target must be active and of the same kind'
            USING ERRCODE='22023';
    END IF;
    IF k=real_key THEN
        RETURN; -- Alias of the canonical spelling itself is unnecessary.
    END IF;
    SELECT id INTO existing_id FROM book_taxonomy_terms
    WHERE kind=desired_kind AND search_key=k;
    IF existing_id IS NOT NULL THEN
        RAISE EXCEPTION 'alias conflicts with a canonical term'
            USING ERRCODE='23505';
    END IF;
    SELECT term_id INTO existing_id FROM book_taxonomy_aliases
    WHERE kind=desired_kind AND search_key=k;
    IF existing_id=target_id THEN
        RETURN; -- Idempotent repeated reviewed decision.
    END IF;
    IF existing_id IS NOT NULL THEN
        RAISE EXCEPTION 'alias belongs to a different term'
            USING ERRCODE='23505';
    END IF;
    INSERT INTO book_taxonomy_aliases(kind,search_key,term_id)
    VALUES(desired_kind,k,target_id);
    INSERT INTO book_taxonomy_curation_events(
        action,kind,term_id,curator,details
    ) VALUES(
        'add_alias',desired_kind,target_id,curator_tag,
        jsonb_build_object('alias_key',k)
    );
END;
$$;

-- Explicit reconciliation for a *single named book*. Unknown/ambiguous
-- labels, blocked terms, duplicates or >8 items abort without losing any
-- legacy raw metadata. All F/REC compatibility arrays and links change in one
-- transaction. No global auto-rewrite, and no implicit merges.
CREATE FUNCTION admin_taxonomy_reconcile_book(
    target_book_id BIGINT, curator_tag TEXT
) RETURNS BOOLEAN LANGUAGE plpgsql SECURITY INVOKER AS $$
DECLARE
    b RECORD;
    cat TEXT;
    labels TEXT[];
    name TEXT;
    key TEXT;
    ids BIGINT[];
    keys TEXT[];
    candidate_ids BIGINT[];
    candidate_keys TEXT[];
    old_ids BIGINT[];
    item_id BIGINT;
    item_key TEXT;
    genre_ids BIGINT[] := ARRAY[]::BIGINT[];
    tag_ids BIGINT[] := ARRAY[]::BIGINT[];
    fandom_ids BIGINT[] := ARRAY[]::BIGINT[];
    genre_keys TEXT[] := ARRAY[]::TEXT[];
    tag_keys TEXT[] := ARRAY[]::TEXT[];
    fandom_keys TEXT[] := ARRAY[]::TEXT[];
    changed BOOLEAN := false;
BEGIN
    PERFORM admin_taxonomy_guard(curator_tag);
    SELECT genres,tags,fandoms,taxonomy_revision,admin_hold,is_deleted
    INTO b FROM books WHERE id=target_book_id FOR UPDATE;
    IF NOT FOUND OR b.admin_hold OR b.is_deleted THEN
        RAISE EXCEPTION 'book does not exist or is restricted'
            USING ERRCODE='22023';
    END IF;
    FOREACH cat IN ARRAY ARRAY['genre','tag','fandom'] LOOP
        labels := CASE cat WHEN 'genre' THEN b.genres
                            WHEN 'tag' THEN b.tags ELSE b.fandoms END;
        labels := coalesce(labels,ARRAY[]::TEXT[]);
        IF cardinality(labels)>8 THEN
            RAISE EXCEPTION 'too many legacy % labels',cat
                USING ERRCODE='22023';
        END IF;
        ids := ARRAY[]::BIGINT[];
        keys := ARRAY[]::TEXT[];
        FOREACH name IN ARRAY labels LOOP
            IF name IS NULL OR name='' OR name ~ '[[:cntrl:]]' THEN
                RAISE EXCEPTION 'invalid legacy % label',cat
                    USING ERRCODE='22023';
            END IF;
            key := admin_taxonomy_key(name);
            SELECT array_agg(t.id ORDER BY t.id),
                   array_agg(t.search_key ORDER BY t.id)
              INTO candidate_ids,candidate_keys
              FROM book_taxonomy_terms t
             WHERE t.kind=cat AND t.status='active'
               AND (t.search_key=key OR EXISTS (
                   SELECT 1 FROM book_taxonomy_aliases a
                   WHERE a.kind=cat AND a.search_key=key AND a.term_id=t.id
               ));
            IF coalesce(cardinality(candidate_ids),0)<>1 THEN
                RAISE EXCEPTION 'unknown or ambiguous legacy % label: %',cat,key
                    USING ERRCODE='22023';
            END IF;
            item_id := candidate_ids[1];
            item_key := candidate_keys[1];
            IF item_id = ANY(ids) THEN
                RAISE EXCEPTION 'duplicate canonical IDs in book %',target_book_id
                    USING ERRCODE='22023';
            END IF;
            ids := array_append(ids,item_id);
            keys := array_append(keys,item_key);
        END LOOP;
        SELECT coalesce(array_agg(term_id ORDER BY position),ARRAY[]::BIGINT[])
          INTO old_ids FROM book_taxonomy_links
         WHERE book_id=target_book_id AND kind=cat;
        changed := changed OR labels IS DISTINCT FROM keys OR old_ids<>ids;
        CASE cat
        WHEN 'genre' THEN genre_ids:=ids; genre_keys:=keys;
        WHEN 'tag' THEN tag_ids:=ids; tag_keys:=keys;
        ELSE fandom_ids:=ids; fandom_keys:=keys;
        END CASE;
    END LOOP;
    IF NOT changed THEN RETURN false; END IF;
    UPDATE books
       SET genres=genre_keys,tags=tag_keys,fandoms=fandom_keys,
           taxonomy_revision=taxonomy_revision+1
     WHERE id=target_book_id;
    DELETE FROM book_taxonomy_links WHERE book_id=target_book_id;
    INSERT INTO book_taxonomy_links(book_id,kind,term_id,position)
    SELECT target_book_id,src.kind,src.term_id,src.ord-1
      FROM (
        SELECT 'genre'::TEXT AS kind,term_id,ord
          FROM unnest(genre_ids) WITH ORDINALITY AS v(term_id,ord)
        UNION ALL
        SELECT 'tag'::TEXT,term_id,ord
          FROM unnest(tag_ids) WITH ORDINALITY AS v(term_id,ord)
        UNION ALL
        SELECT 'fandom'::TEXT,term_id,ord
          FROM unnest(fandom_ids) WITH ORDINALITY AS v(term_id,ord)
      ) src;
    INSERT INTO book_taxonomy_curation_events(action,book_id,curator,details)
    VALUES('reconcile_book',target_book_id,curator_tag,
        jsonb_build_object('genres',cardinality(genre_ids),
                           'tags',cardinality(tag_ids),
                           'fandoms',cardinality(fandom_ids)));
    RETURN true;
END;
$$;

REVOKE ALL ON book_taxonomy_curation_events FROM PUBLIC;
REVOKE ALL ON SEQUENCE book_taxonomy_curation_events_id_seq FROM PUBLIC;
REVOKE ALL ON FUNCTION admin_taxonomy_key(TEXT),
    admin_taxonomy_guard(TEXT),
    admin_taxonomy_add_term(TEXT,TEXT,TEXT),
    admin_taxonomy_add_alias(TEXT,BIGINT,TEXT,TEXT),
    admin_taxonomy_reconcile_book(BIGINT,TEXT) FROM PUBLIC;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname='tor_app') THEN
        REVOKE ALL ON book_taxonomy_curation_events FROM tor_app;
        REVOKE ALL ON SEQUENCE book_taxonomy_curation_events_id_seq FROM tor_app;
        REVOKE ALL ON FUNCTION admin_taxonomy_key(TEXT),
            admin_taxonomy_guard(TEXT),
            admin_taxonomy_add_term(TEXT,TEXT,TEXT),
            admin_taxonomy_add_alias(TEXT,BIGINT,TEXT,TEXT),
            admin_taxonomy_reconcile_book(BIGINT,TEXT) FROM tor_app;
    END IF;
END
$$;
