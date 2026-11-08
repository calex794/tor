-- S020: anonymous catalog, search facets and counts share one public-basic rule.
-- Repo-only migration: no rollout to production is part of this change.
-- SQLx wraps migration in one transaction; lock ensures no transitional deltas
-- can race with the exact backfill, after which existing triggers call these
-- revised SECURITY DEFINER functions using their transition tables.
LOCK TABLE books IN SHARE ROW EXCLUSIVE MODE;

UPDATE public_catalog_statistics
   SET visible_book_count = (
      SELECT COUNT(*)::bigint FROM books WHERE visibility='public' AND visibility_status='active' AND access_level='basic' AND NOT admin_hold AND NOT is_deleted
   )
 WHERE singleton;

DELETE FROM public_catalog_facets;
INSERT INTO public_catalog_facets(kind,name,reference_count)
SELECT kind,name,COUNT(*)::bigint
FROM (
  SELECT DISTINCT b.id,f.kind,item.name
  FROM books AS b
  CROSS JOIN LATERAL (
    VALUES ('genre'::text,b.genres),
           ('tag'::text,b.tags),
           ('fandom'::text,b.fandoms)
  ) AS f(kind,labels)
  CROSS JOIN LATERAL unnest(f.labels) AS item(name)
  WHERE b.visibility='public' AND b.visibility_status='active' AND b.access_level='basic' AND NOT b.admin_hold AND NOT b.is_deleted
) AS labels
GROUP BY kind,name;

CREATE OR REPLACE FUNCTION public.perf233_after_book_insert()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog,public,pg_temp AS $$
DECLARE total BIGINT; facet RECORD;
BEGIN
    SELECT COUNT(*) INTO total FROM new_rows
    WHERE visibility='public' AND visibility_status='active' AND access_level='basic' AND NOT admin_hold AND NOT is_deleted;
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
            WHERE b.visibility='public' AND b.visibility_status='active' AND b.access_level='basic' AND NOT b.admin_hold AND NOT b.is_deleted
        ) AS labels
        GROUP BY kind,name ORDER BY kind,name
    LOOP
        PERFORM public.perf233_adjust_facet(facet.kind,facet.name,facet.change);
    END LOOP;
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION public.perf233_after_book_delete()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog,public,pg_temp AS $$
DECLARE total BIGINT; facet RECORD;
BEGIN
    SELECT COUNT(*) INTO total FROM old_rows
    WHERE visibility='public' AND visibility_status='active' AND access_level='basic' AND NOT admin_hold AND NOT is_deleted;
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
            WHERE b.visibility='public' AND b.visibility_status='active' AND b.access_level='basic' AND NOT b.admin_hold AND NOT b.is_deleted
        ) AS labels
        GROUP BY kind,name ORDER BY kind,name
    LOOP
        PERFORM public.perf233_adjust_facet(facet.kind,facet.name,-facet.change);
    END LOOP;
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION public.perf233_after_book_update()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog,public,pg_temp AS $$
DECLARE total BIGINT; facet RECORD;
BEGIN
    SELECT
      (SELECT COUNT(*) FROM new_rows WHERE visibility='public' AND visibility_status='active' AND access_level='basic' AND NOT admin_hold AND NOT is_deleted)
      -
      (SELECT COUNT(*) FROM old_rows WHERE visibility='public' AND visibility_status='active' AND access_level='basic' AND NOT admin_hold AND NOT is_deleted)
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
            WHERE b.visibility='public' AND b.visibility_status='active' AND b.access_level='basic' AND NOT b.admin_hold AND NOT b.is_deleted
            UNION ALL
            SELECT DISTINCT b.id,f.kind,item.name,1::BIGINT AS delta
            FROM new_rows AS b
            CROSS JOIN LATERAL (
                VALUES ('genre'::TEXT,b.genres),
                       ('tag'::TEXT,b.tags),
                       ('fandom'::TEXT,b.fandoms)
            ) AS f(kind,labels)
            CROSS JOIN LATERAL unnest(f.labels) AS item(name)
            WHERE b.visibility='public' AND b.visibility_status='active' AND b.access_level='basic' AND NOT b.admin_hold AND NOT b.is_deleted
        ) AS changed
        GROUP BY kind,name HAVING SUM(delta)<>0 ORDER BY kind,name
    LOOP
        PERFORM public.perf233_adjust_facet(facet.kind,facet.name,facet.change);
    END LOOP;
    RETURN NULL;
END;
$$;
