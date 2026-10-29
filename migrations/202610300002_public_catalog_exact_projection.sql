-- PERF-233: exact transactionally maintained public catalogue projections.
-- A public listing's first page must not scan every book to derive totals or
-- genres/tags/fandoms. This is NOT a TTL cache: PostgreSQL triggers update
-- these counts in the same transaction as visibility and metadata writes.
-- Statement-level transition tables make a 100k-book import a bounded batch.
-- The tor_app role receives SELECT only; mutations run as migration owner.
-- SQLx applies this migration transactionally: hold writes while backfilling,
-- or a commit between the snapshot and trigger creation could lose a delta.
LOCK TABLE books IN SHARE ROW EXCLUSIVE MODE;
CREATE TABLE public_catalog_statistics (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    visible_book_count BIGINT NOT NULL CHECK (visible_book_count >= 0)
);
INSERT INTO public_catalog_statistics (singleton, visible_book_count)
SELECT TRUE, COUNT(*)::BIGINT FROM books
WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;

CREATE TABLE public_catalog_facets (
    kind TEXT NOT NULL CHECK (kind IN ('genre','tag','fandom')),
    name TEXT NOT NULL CHECK (name <> ''),
    reference_count BIGINT NOT NULL CHECK (reference_count >= 0),
    PRIMARY KEY (kind,name)
);
-- Deduplicate repeated values inside one book, as the existing DISTINCT did.
INSERT INTO public_catalog_facets (kind,name,reference_count)
SELECT kind,name,COUNT(*)::BIGINT
FROM (
    SELECT DISTINCT b.id,f.kind,item.name
    FROM books AS b
    CROSS JOIN LATERAL (
        VALUES ('genre'::TEXT,b.genres),
               ('tag'::TEXT,b.tags),
               ('fandom'::TEXT,b.fandoms)
    ) AS f(kind,labels)
    CROSS JOIN LATERAL unnest(f.labels) AS item(name)
    WHERE b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
) AS labels
GROUP BY kind,name;

-- No API credentials can write fabricated counts or call the privileged
-- delta helpers directly. Trigger owners can access them on behalf of writes.
REVOKE ALL ON public_catalog_statistics,public_catalog_facets FROM PUBLIC;
REVOKE INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER
    ON public_catalog_statistics,public_catalog_facets FROM tor_app;
GRANT SELECT ON public_catalog_statistics,public_catalog_facets TO tor_app;

CREATE FUNCTION public.perf233_adjust_count(change BIGINT)
RETURNS VOID LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog,public,pg_temp AS $
BEGIN
    IF change = 0 THEN RETURN; END IF;
    UPDATE public.public_catalog_statistics
       SET visible_book_count = visible_book_count + change
     WHERE singleton AND visible_book_count + change >= 0;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'public catalogue total would be inconsistent'
            USING ERRCODE='23514';
    END IF;
END;
$$;

CREATE FUNCTION public.perf233_adjust_facet(facet_kind TEXT,facet_name TEXT,change BIGINT)
RETURNS VOID LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog,public,pg_temp AS $
BEGIN
    IF change = 0 THEN RETURN; END IF;
    IF change > 0 THEN
        INSERT INTO public.public_catalog_facets(kind,name,reference_count)
        VALUES (facet_kind,facet_name,change)
        ON CONFLICT (kind,name) DO UPDATE
          SET reference_count =
              public_catalog_facets.reference_count + EXCLUDED.reference_count;
    ELSE
        UPDATE public.public_catalog_facets
           SET reference_count = reference_count + change
         WHERE kind=facet_kind AND name=facet_name
           AND reference_count + change >= 0;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'public facet count would be inconsistent'
                USING ERRCODE='23514';
        END IF;
        DELETE FROM public.public_catalog_facets
         WHERE kind=facet_kind AND name=facet_name AND reference_count=0;
    END IF;
END;
$$;

CREATE FUNCTION public.perf233_after_book_insert()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog,public,pg_temp AS $
DECLARE total BIGINT; facet RECORD;
BEGIN
    SELECT COUNT(*) INTO total FROM new_rows
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;
    PERFORM public.perf233_adjust_count(total);
    FOR facet IN
        SELECT kind,name,COUNT(*)::BIGINT AS change FROM (
            SELECT DISTINCT b.id,f.kind,item.name
            FROM new_rows AS b
            CROSS JOIN LATERAL (
                VALUES ('genre'::TEXT,b.genres),
                       ('tag'::TEXT,b.tags),
                       ('fandom'::TEXT,b.fandoms)
            ) AS f(kind,labels)
            CROSS JOIN LATERAL unnest(f.labels) AS item(name)
            WHERE b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
        ) AS labels
        GROUP BY kind,name ORDER BY kind,name
    LOOP
        PERFORM public.perf233_adjust_facet(facet.kind,facet.name,facet.change);
    END LOOP;
    RETURN NULL;
END;
$$;

CREATE FUNCTION public.perf233_after_book_delete()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog,public,pg_temp AS $
DECLARE total BIGINT; facet RECORD;
BEGIN
    SELECT COUNT(*) INTO total FROM old_rows
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;
    PERFORM public.perf233_adjust_count(-total);
    FOR facet IN
        SELECT kind,name,COUNT(*)::BIGINT AS change FROM (
            SELECT DISTINCT b.id,f.kind,item.name
            FROM old_rows AS b
            CROSS JOIN LATERAL (
                VALUES ('genre'::TEXT,b.genres),
                       ('tag'::TEXT,b.tags),
                       ('fandom'::TEXT,b.fandoms)
            ) AS f(kind,labels)
            CROSS JOIN LATERAL unnest(f.labels) AS item(name)
            WHERE b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
        ) AS labels
        GROUP BY kind,name ORDER BY kind,name
    LOOP
        PERFORM public.perf233_adjust_facet(facet.kind,facet.name,-facet.change);
    END LOOP;
    RETURN NULL;
END;
$$;

CREATE FUNCTION public.perf233_after_book_update()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog,public,pg_temp AS $
DECLARE total BIGINT; facet RECORD;
BEGIN
    SELECT
      (SELECT COUNT(*) FROM new_rows WHERE visibility='public'
          AND NOT admin_hold AND NOT is_deleted)
      -
      (SELECT COUNT(*) FROM old_rows WHERE visibility='public'
          AND NOT admin_hold AND NOT is_deleted)
    INTO total;
    PERFORM public.perf233_adjust_count(total);
    FOR facet IN
        SELECT kind,name,SUM(delta)::BIGINT AS change FROM (
            SELECT DISTINCT b.id,f.kind,item.name,-1::BIGINT AS delta
            FROM old_rows AS b
            CROSS JOIN LATERAL (
                VALUES ('genre'::TEXT,b.genres),
                       ('tag'::TEXT,b.tags),
                       ('fandom'::TEXT,b.fandoms)
            ) AS f(kind,labels)
            CROSS JOIN LATERAL unnest(f.labels) AS item(name)
            WHERE b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
            UNION ALL
            SELECT DISTINCT b.id,f.kind,item.name,1::BIGINT AS delta
            FROM new_rows AS b
            CROSS JOIN LATERAL (
                VALUES ('genre'::TEXT,b.genres),
                       ('tag'::TEXT,b.tags),
                       ('fandom'::TEXT,b.fandoms)
            ) AS f(kind,labels)
            CROSS JOIN LATERAL unnest(f.labels) AS item(name)
            WHERE b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
        ) AS changed
        GROUP BY kind,name HAVING SUM(delta)<>0 ORDER BY kind,name
    LOOP
        PERFORM public.perf233_adjust_facet(facet.kind,facet.name,facet.change);
    END LOOP;
    RETURN NULL;
END;
$$;

-- CREATE TRIGGER permissions are checked at DDL time; runtime tor_app only
-- needs permission to mutate its original books rows, not the projections.
CREATE TRIGGER perf233_sync_public_insert
AFTER INSERT ON books REFERENCING NEW TABLE AS new_rows
FOR EACH STATEMENT EXECUTE FUNCTION public.perf233_after_book_insert();
CREATE TRIGGER perf233_sync_public_update
AFTER UPDATE ON books REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows
FOR EACH STATEMENT EXECUTE FUNCTION public.perf233_after_book_update();
CREATE TRIGGER perf233_sync_public_delete
AFTER DELETE ON books REFERENCING OLD TABLE AS old_rows
FOR EACH STATEMENT EXECUTE FUNCTION public.perf233_after_book_delete();

REVOKE ALL ON FUNCTION public.perf233_adjust_count(BIGINT) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.perf233_adjust_facet(TEXT,TEXT,BIGINT) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.perf233_after_book_insert() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.perf233_after_book_update() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.perf233_after_book_delete() FROM PUBLIC;
