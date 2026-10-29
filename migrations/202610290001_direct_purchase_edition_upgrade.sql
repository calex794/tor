-- D / NEwBOO: a paid purchase is immutable, but its selected library
-- edition may advance to the current, separately verified publication.
-- Original chapter_purchase_facts.version_id is never rewritten.
ALTER TABLE chapter_purchase_facts
    ADD CONSTRAINT chapter_purchase_owned_identity_unique
    UNIQUE (id, user_id, chapter_id);

-- The old FK incorrectly equates a selected library edition to the
-- historical settlement's version. Keep ownership and version FKs separate.
DO $$
DECLARE old_constraint TEXT;
BEGIN
    SELECT conname INTO old_constraint
    FROM pg_constraint
    WHERE conrelid = 'library_purchased_chapters'::regclass
      AND confrelid = 'chapter_purchase_facts'::regclass
      AND contype = 'f'
      AND pg_get_constraintdef(oid) LIKE '%active_version_id%';
    IF old_constraint IS NULL THEN
        RAISE EXCEPTION 'historical library purchase FK not found';
    END IF;
    EXECUTE format(
        'ALTER TABLE library_purchased_chapters DROP CONSTRAINT %I',
        old_constraint
    );
END;
$$;

ALTER TABLE library_purchased_chapters
    ADD CONSTRAINT library_purchase_anchor_fkey
        FOREIGN KEY (purchase_id, user_id, chapter_id)
        REFERENCES chapter_purchase_facts (id, user_id, chapter_id)
        ON DELETE RESTRICT,
    ADD CONSTRAINT library_selected_edition_fkey
        FOREIGN KEY (chapter_id, active_version_id)
        REFERENCES chapter_versions (chapter_id, id)
        ON DELETE RESTRICT;

DROP TRIGGER purchased_library_no_mutation ON library_purchased_chapters;

-- A compromised writer cannot move a library pointer to an unverified
-- draft, foreign chapter, refunded purchase or arbitrary historical body.
CREATE FUNCTION guard_library_selected_edition()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'purchased library entries cannot be deleted'
            USING ERRCODE = '23514';
    END IF;
    IF (NEW.user_id, NEW.chapter_id, NEW.purchase_id, NEW.created_at)
        IS DISTINCT FROM
       (OLD.user_id, OLD.chapter_id, OLD.purchase_id, OLD.created_at) THEN
        RAISE EXCEPTION 'purchase identity and history cannot be changed'
            USING ERRCODE = '23514';
    END IF;
    IF NEW.active_version_id = OLD.active_version_id THEN
        RETURN NEW;
    END IF;

    IF NOT EXISTS (
        SELECT 1
        FROM chapter_purchase_facts purchase
        JOIN chapters chapter ON chapter.id = purchase.chapter_id
        JOIN books book ON book.id = chapter.book_id
        JOIN chapter_publications publication
          ON publication.chapter_id = chapter.id
         AND publication.version_id = NEW.active_version_id
        JOIN chapter_versions edition
          ON edition.chapter_id = chapter.id
         AND edition.id = publication.version_id
         AND edition.storage_object_id IS NOT NULL
        JOIN chapter_encrypted_release_events release
          ON release.chapter_id = edition.chapter_id
         AND release.version_id = edition.id
         AND release.storage_object_id = edition.storage_object_id
        JOIN storage_objects stored
          ON stored.id = edition.storage_object_id AND stored.state = 'attached'
        WHERE purchase.id = NEW.purchase_id
          AND purchase.user_id = NEW.user_id
          AND purchase.chapter_id = NEW.chapter_id
          AND book.visibility = 'public'
          AND chapter.visibility = 'public'
          AND chapter.access_level = 'paid'
          AND chapter.price_amount > 0
          AND NOT book.admin_hold AND NOT book.is_deleted
          AND NOT chapter.admin_hold AND NOT chapter.is_deleted
          AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at <= now())
          AND NOT EXISTS (
              SELECT 1 FROM chapter_purchase_revocations revoked
              WHERE revoked.purchase_id = purchase.id
          )
    ) THEN
        RAISE EXCEPTION 'selected edition is not an authorized verified publication'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER purchased_library_guard_selected_edition
    BEFORE UPDATE OR DELETE ON library_purchased_chapters
    FOR EACH ROW EXECUTE FUNCTION guard_library_selected_edition();

-- Read only the user's selected, same-chapter edition. A historical
-- purchased V1 remains readable after hiding a storefront; a newly
-- selected V2 must have a B/G verified encrypted release fact.
CREATE OR REPLACE FUNCTION tor_purchase_version_readable(
    actor_id BIGINT, target_purchase BIGINT, target_chapter BIGINT, target_version BIGINT
) RETURNS BOOLEAN LANGUAGE SQL STABLE SECURITY INVOKER
SET search_path = pg_catalog, public AS $$
    SELECT EXISTS (
        SELECT 1
        FROM public.chapter_purchase_facts AS purchased
        JOIN public.library_purchased_chapters AS owned
          ON owned.purchase_id = purchased.id
         AND owned.user_id = purchased.user_id
         AND owned.chapter_id = purchased.chapter_id
        JOIN public.chapter_versions AS edition
          ON edition.id = owned.active_version_id
         AND edition.chapter_id = purchased.chapter_id
        JOIN public.chapters AS chapter ON chapter.id = purchased.chapter_id
        JOIN public.books AS book ON book.id = chapter.book_id
        LEFT JOIN public.chapter_purchase_revocations AS revoked
          ON revoked.purchase_id = purchased.id
        WHERE purchased.id = target_purchase
          AND purchased.user_id = actor_id
          AND purchased.chapter_id = target_chapter
          AND owned.active_version_id = target_version
          AND revoked.purchase_id IS NULL
          AND NOT book.admin_hold AND NOT book.is_deleted
          AND NOT chapter.admin_hold AND NOT chapter.is_deleted
          AND (
              owned.active_version_id = purchased.version_id
              OR EXISTS (
                  SELECT 1
                  FROM public.chapter_encrypted_release_events AS released
                  JOIN public.storage_objects AS object
                    ON object.id = released.storage_object_id
                   AND object.state = 'attached'
                  WHERE released.chapter_id = purchased.chapter_id
                    AND released.version_id = owned.active_version_id
                    AND released.storage_object_id = edition.storage_object_id
              )
          )
    );
$$;

-- Trusted D operation: one atomic, owner-scoped selection of the currently
-- published paid edition. The client never chooses a version or S3 object.
-- PostgreSQL serializes competing requests on the library row; the guard
-- above checks B/G verified release membership again before committing.
CREATE FUNCTION tor_select_latest_purchased_edition(
    actor_id BIGINT, target_purchase BIGINT, target_chapter BIGINT
) RETURNS BIGINT LANGUAGE plpgsql VOLATILE SECURITY INVOKER
SET search_path = pg_catalog, public AS $$
DECLARE
    selected BIGINT;
    latest BIGINT;
BEGIN
    SELECT owned.active_version_id INTO selected
    FROM public.library_purchased_chapters AS owned
    WHERE owned.user_id = actor_id
      AND owned.purchase_id = target_purchase
      AND owned.chapter_id = target_chapter
    FOR UPDATE;
    IF selected IS NULL THEN
        RETURN NULL;
    END IF;
    IF NOT public.tor_purchase_version_readable(
        actor_id, target_purchase, target_chapter, selected
    ) THEN
        RETURN NULL;
    END IF;

    SELECT publication.version_id INTO latest
    FROM public.chapter_publications AS publication
    JOIN public.chapters AS chapter ON chapter.id = publication.chapter_id
    JOIN public.books AS book ON book.id = chapter.book_id
    JOIN public.chapter_versions AS edition
      ON edition.id = publication.version_id
     AND edition.chapter_id = publication.chapter_id
    JOIN public.chapter_encrypted_release_events AS release
      ON release.chapter_id = edition.chapter_id
     AND release.version_id = edition.id
     AND release.storage_object_id = edition.storage_object_id
    JOIN public.storage_objects AS stored
      ON stored.id = edition.storage_object_id AND stored.state = 'attached'
    WHERE publication.chapter_id = target_chapter
      AND book.visibility = 'public'
      AND chapter.visibility = 'public'
      AND chapter.access_level = 'paid'
      AND chapter.price_amount > 0
      AND NOT book.admin_hold AND NOT book.is_deleted
      AND NOT chapter.admin_hold AND NOT chapter.is_deleted
      AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at <= now())
      AND NOT EXISTS (
          SELECT 1 FROM public.chapter_purchase_revocations AS revoked
          WHERE revoked.purchase_id = target_purchase
      );
    IF latest IS NULL THEN
        RETURN selected;
    END IF;
    -- Never downgrade: a concurrent editor could have published an older
    -- pointer, but this library must not move backwards.
    IF latest <= selected THEN
        RETURN selected;
    END IF;
    UPDATE public.library_purchased_chapters
    SET active_version_id = latest
    WHERE user_id = actor_id
      AND chapter_id = target_chapter
      AND purchase_id = target_purchase;
    RETURN latest;
END;
$$;
