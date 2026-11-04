-- S020/#486: authoritative D purchase check also validates the *current* age tier.
-- A purchase creates possession, not an irrevocable age exemption. Do not
-- mutate settlement facts, pinned library pointers, refunds, or the paid body.
-- Preserve NEwBOO hidden/draft/pending_deletion pinned entitlement, but
-- 'blocked', deletion and admin hold remain hard denial. No production rollout.
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
        JOIN public.users AS viewer ON viewer.id=purchased.user_id AND viewer.account_state='active'
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
          -- NEwBOO: 'blocked' is a hard denial, whereas hidden/pending_deletion
          -- preserve already purchased pinned editions until actual deletion.
          AND book.visibility_status<>'blocked'
          AND (
              book.access_level='basic'
              OR (book.access_level='adult' AND viewer.adult_enabled)
              OR (
                  book.access_level='pro_adult'
                  AND EXISTS (
                      SELECT 1 FROM public.user_level_grants AS age_grant
                      WHERE age_grant.user_id=viewer.id
                        AND age_grant.level='pro_adult' AND age_grant.is_active
                  )
              )
          )
          AND NOT chapter.admin_hold AND NOT chapter.is_deleted
    );
$$;
