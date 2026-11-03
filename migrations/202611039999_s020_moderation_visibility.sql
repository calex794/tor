-- S020: NEwBOO four-state moderation visibility, independent of draft/public.
-- Runs only when SQLx applies this committed migration. No production rollout.
LOCK TABLE books IN SHARE ROW EXCLUSIVE MODE;
ALTER TABLE books
    ADD COLUMN visibility_status TEXT NOT NULL DEFAULT 'active'
        CHECK (visibility_status IN ('active','hidden','pending_moderation','blocked')),
    ADD COLUMN visibility_status_revision BIGINT NOT NULL DEFAULT 1
        CHECK (visibility_status_revision > 0);
CREATE INDEX books_public_moderation_visible_id_idx ON books (id)
    WHERE visibility='public' AND visibility_status='active'
      AND NOT admin_hold AND NOT is_deleted;

-- Existing books default active. Exact catalogue statistics and facets remain
-- unchanged at installation. Statement-level triggers now apply status deltas.
CREATE OR REPLACE FUNCTION public.perf233_after_book_insert()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog,public,pg_temp AS $$
DECLARE total BIGINT; facet RECORD;
BEGIN
    SELECT COUNT(*) INTO total FROM new_rows
    WHERE visibility='public' AND visibility_status='active' AND NOT admin_hold AND NOT is_deleted;
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
            WHERE b.visibility='public' AND b.visibility_status='active' AND NOT b.admin_hold AND NOT b.is_deleted
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
    WHERE visibility='public' AND visibility_status='active' AND NOT admin_hold AND NOT is_deleted;
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
            WHERE b.visibility='public' AND b.visibility_status='active' AND NOT b.admin_hold AND NOT b.is_deleted
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
      (SELECT COUNT(*) FROM new_rows WHERE visibility='public' AND visibility_status='active'
          AND NOT admin_hold AND NOT is_deleted)
      -
      (SELECT COUNT(*) FROM old_rows WHERE visibility='public' AND visibility_status='active'
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
            WHERE b.visibility='public' AND b.visibility_status='active' AND NOT b.admin_hold AND NOT b.is_deleted
            UNION ALL
            SELECT DISTINCT b.id,f.kind,item.name,1::BIGINT AS delta
            FROM new_rows AS b
            CROSS JOIN LATERAL (
                VALUES ('genre'::TEXT,b.genres),
                       ('tag'::TEXT,b.tags),
                       ('fandom'::TEXT,b.fandoms)
            ) AS f(kind,labels)
            CROSS JOIN LATERAL unnest(f.labels) AS item(name)
            WHERE b.visibility='public' AND b.visibility_status='active' AND NOT b.admin_hold AND NOT b.is_deleted
        ) AS changed
        GROUP BY kind,name HAVING SUM(delta)<>0 ORDER BY kind,name
    LOOP
        PERFORM public.perf233_adjust_facet(facet.kind,facet.name,facet.change);
    END LOOP;
    RETURN NULL;
END;
$$;

-- Security-barrier projections used by recommendations and shadow reading.
CREATE OR REPLACE VIEW shadow_public_books WITH (security_barrier = true) AS
SELECT id, source_identity, target_language, title, visibility,
       author_name, genres, seo_index_owner
FROM books
WHERE shadow_enabled
  AND visibility='public' AND visibility_status='active' AND NOT admin_hold AND NOT is_deleted;

CREATE OR REPLACE VIEW shadow_public_chapters WITH (security_barrier = true) AS
SELECT c.id, c.book_id, c.chapter_no, c.title
FROM chapters AS c
JOIN books AS b ON b.id = c.book_id
JOIN chapter_publications AS p ON p.chapter_id = c.id
WHERE b.shadow_enabled
  AND b.visibility='public' AND b.visibility_status='active' AND NOT b.admin_hold AND NOT b.is_deleted
  AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
  AND (c.reader_released_at IS NULL OR c.reader_released_at <= now());

CREATE OR REPLACE VIEW rec_public_books WITH (security_barrier=true) AS
SELECT b.id,b.title,b.target_language,b.author_name,b.genres,b.tags,b.created_at
FROM books b LEFT JOIN book_recommendation_audiences a ON a.book_id=b.id
WHERE b.visibility='public' AND b.visibility_status='active' AND NOT b.admin_hold AND NOT b.is_deleted
  AND b.access_level='basic'
  AND COALESCE(a.audience,'general')='general';

-- Following a book or creator must not reveal hidden moderation-state titles.
-- Both UNION branches retain their original release and entitlement predicates.
CREATE OR REPLACE VIEW reader_followed_release_candidates AS
SELECT follow.user_id,
       book.id AS book_id,
       book.title AS book_title,
       chapter.id AS chapter_id,
       chapter.title AS chapter_title,
       chapter.chapter_no AS chapter_number,
       chapter.access_level,
       chapter.price_amount AS price_minor,
       release.created_at AS released_at,
       (EXTRACT(EPOCH FROM release.created_at) * 1000000)::bigint AS released_micros
FROM reader_book_follows AS follow
JOIN books AS book ON book.id=follow.book_id
JOIN chapters AS chapter ON chapter.book_id=book.id
JOIN chapter_first_release_events AS release ON release.chapter_id=chapter.id
JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id
WHERE release.created_at>=follow.created_at
  AND book.visibility='public' AND book.visibility_status='active'
  AND NOT book.admin_hold AND NOT book.is_deleted
  AND chapter.visibility='public'
  AND NOT chapter.admin_hold AND NOT chapter.is_deleted
  AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at<=now())
  AND (
      (chapter.access_level='free' AND chapter.price_amount=0)
      OR (
          chapter.access_level='paid' AND chapter.price_amount>0
          AND EXISTS (
              SELECT 1
              FROM chapter_encrypted_release_events AS encrypted
              JOIN chapter_versions AS current_version
                ON current_version.id=encrypted.version_id
               AND current_version.chapter_id=encrypted.chapter_id
              JOIN storage_objects AS object
                ON object.id=encrypted.storage_object_id
               AND object.id=current_version.storage_object_id
              WHERE encrypted.chapter_id=chapter.id
                AND encrypted.version_id=publication.version_id
                AND object.state='attached'
                AND object.object_kind='chapter'
          )
      )
  )
UNION
SELECT follow.follower_user_id AS user_id,
       book.id AS book_id,
       book.title AS book_title,
       chapter.id AS chapter_id,
       chapter.title AS chapter_title,
       chapter.chapter_no AS chapter_number,
       chapter.access_level,
       chapter.price_amount AS price_minor,
       release.created_at AS released_at,
       (EXTRACT(EPOCH FROM release.created_at) * 1000000)::bigint AS released_micros
FROM creator_follow_periods AS follow
JOIN chapter_first_release_events AS release
  ON release.creator_user_id=follow.creator_user_id
 AND release.created_at>=follow.started_at
 AND (follow.ended_at IS NULL OR release.created_at<follow.ended_at)
JOIN chapters AS chapter ON chapter.id=release.chapter_id
JOIN books AS book ON book.id=chapter.book_id
JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id
WHERE book.visibility='public' AND book.visibility_status='active'
  AND NOT book.admin_hold AND NOT book.is_deleted
  AND chapter.visibility='public'
  AND NOT chapter.admin_hold AND NOT chapter.is_deleted
  AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at<=now())
  AND (
      (chapter.access_level='free' AND chapter.price_amount=0)
      OR (
          chapter.access_level='paid' AND chapter.price_amount>0
          AND EXISTS (
              SELECT 1
              FROM chapter_encrypted_release_events AS encrypted
              JOIN chapter_versions AS current_version
                ON current_version.id=encrypted.version_id
               AND current_version.chapter_id=encrypted.chapter_id
              JOIN storage_objects AS object
                ON object.id=encrypted.storage_object_id
               AND object.id=current_version.storage_object_id
              WHERE encrypted.chapter_id=chapter.id
                AND encrypted.version_id=publication.version_id
                AND object.state='attached'
                AND object.object_kind='chapter'
          )
      )
  );

CREATE TABLE moderation_book_visibility_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_user_id BIGINT NOT NULL CHECK (actor_user_id > 0),
    book_id BIGINT NOT NULL CHECK (book_id > 0),
    before_status TEXT NOT NULL
        CHECK (before_status IN ('active','hidden','pending_moderation','blocked')),
    after_status TEXT NOT NULL
        CHECK (after_status IN ('active','hidden','pending_moderation','blocked')),
    before_revision BIGINT NOT NULL CHECK (before_revision > 0),
    after_revision BIGINT NOT NULL CHECK (after_revision = before_revision + 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (before_status <> after_status)
);
CREATE INDEX moderation_book_visibility_events_recent
    ON moderation_book_visibility_events (book_id,id DESC);
CREATE INDEX moderation_book_visibility_events_actor
    ON moderation_book_visibility_events (actor_user_id,id DESC);
CREATE FUNCTION moderation_book_visibility_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'moderation visibility event is immutable'
        USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER moderation_book_visibility_events_immutable
    BEFORE UPDATE OR DELETE ON moderation_book_visibility_events
    FOR EACH ROW EXECUTE FUNCTION moderation_book_visibility_immutable();
REVOKE ALL ON FUNCTION moderation_book_visibility_immutable() FROM PUBLIC;
