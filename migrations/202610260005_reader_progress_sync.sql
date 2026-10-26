-- C-09/S006: optimistic concurrency for the existing free reading bookmark.
-- Historical migrations remain immutable; legacy HTML saves continue to work.
ALTER TABLE reader_book_positions
    ADD COLUMN revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0);

-- C-09 / BKR-134: purchased version cursor belongs to the exact settlement fact.
-- It does not create or change purchases, refunds or access rights.
CREATE TABLE reader_purchased_positions (
    user_id BIGINT NOT NULL,
    purchase_id BIGINT NOT NULL,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    block_index INTEGER NOT NULL CHECK (block_index BETWEEN 1 AND 200),
    char_offset INTEGER NOT NULL DEFAULT 0 CHECK (char_offset BETWEEN 0 AND 100000),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, purchase_id),
    FOREIGN KEY (purchase_id, user_id, chapter_id, version_id)
        REFERENCES chapter_purchase_facts(id, user_id, chapter_id, version_id)
        ON DELETE RESTRICT
);
CREATE INDEX reader_purchased_positions_recent
    ON reader_purchased_positions (user_id, updated_at DESC, purchase_id DESC);

-- I/D-owned access contract. C calls one restricted function, not D tables.
-- The purchase pins V1 after a publication moves to V2. Revocation and hard
-- holds must be checked for EVERY read and write, never inferred from progress.
CREATE FUNCTION tor_purchase_version_readable(
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
         AND owned.active_version_id = purchased.version_id
        JOIN public.chapters AS chapter ON chapter.id = purchased.chapter_id
        JOIN public.books AS book ON book.id = chapter.book_id
        LEFT JOIN public.chapter_purchase_revocations AS revoked
          ON revoked.purchase_id = purchased.id
        WHERE purchased.id = target_purchase
          AND purchased.user_id = actor_id
          AND purchased.chapter_id = target_chapter
          AND purchased.version_id = target_version
          AND revoked.purchase_id IS NULL
          AND book.visibility = 'public' AND NOT book.admin_hold AND NOT book.is_deleted
          AND chapter.visibility = 'public' AND chapter.access_level = 'paid'
          AND NOT chapter.admin_hold AND NOT chapter.is_deleted
          AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at <= now())
    );
$$;
