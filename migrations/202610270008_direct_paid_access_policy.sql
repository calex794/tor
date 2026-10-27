-- D-03/C-06: preserve a directly paid pinned V1 after the creator hides a
-- storefront or withdraws new sales. Moderation, hard deletion and refund
-- remain hard denial. This is an additive replacement of the existing
-- tor_purchase_version_readable SQL policy, not an edit of historical DDL.
-- Reader progress and user-provided IDs never create an entitlement.
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
          AND NOT book.admin_hold AND NOT book.is_deleted
          AND NOT chapter.admin_hold AND NOT chapter.is_deleted
    );
$$;
