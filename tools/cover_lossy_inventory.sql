-- G-05 / WebP q85: read-only inventory of cover bytes recorded in PostgreSQL.
-- This is metadata, NOT an S3 LIST/HEAD and NOT proof that the source is present.
-- No book ids, user ids, object keys, hashes or source URLs are printed.
-- Usage (with least-privileged PG* environment variables):
--   psql -X -v ON_ERROR_STOP=1 -f tools/cover_lossy_inventory.sql
-- DO NOT run a conversion or S3 deletion based on these aggregates alone.
BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '15s';
SET LOCAL lock_timeout = '2s';

-- 1. Current slots only; no historical candidate may be mistaken for a
-- currently approved/public cover. Sum actual recorded main/card output sizes.
WITH variant_totals AS (
    SELECT job_id,
           COUNT(*) FILTER (WHERE variant_name IN ('main','card')) AS variants,
           COALESCE(SUM(size_bytes) FILTER (WHERE variant_name='main'), 0)::bigint AS main_bytes,
           COALESCE(SUM(size_bytes) FILTER (WHERE variant_name='card'), 0)::bigint AS card_bytes
      FROM cover_variants
     GROUP BY job_id
), attached AS (
    SELECT slot.book_id,
           job.recipe_version,
           job.status AS job_status,
           source.status AS source_status,
           source.size_bytes AS recorded_source_bytes,
           vt.variants, vt.main_bytes, vt.card_bytes,
           slot.is_withdrawn,
           review.state AS review_state,
           EXISTS (
               SELECT 1 FROM book_cover_review_requests pending
                WHERE pending.book_id=slot.book_id AND pending.state='pending'
           ) AS pending_review,
           (book.visibility='public' AND book.visibility_status='active'
                AND book.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
                AND book.access_level='basic' AND NOT book.admin_hold
                AND NOT book.is_deleted AND NOT slot.is_withdrawn
                AND book.owner_user_id=job.actor_user_id
                AND job.status='done' AND review.state='approved'
           ) AS currently_public_and_approved
      FROM book_cover_slots slot
      JOIN books book ON book.id=slot.book_id
      JOIN cover_variant_jobs job ON job.id=slot.job_id AND job.book_id=slot.book_id
      JOIN cover_upload_objects source ON source.id=job.source_object_id
      LEFT JOIN book_cover_review_requests review ON review.job_id=job.id
      LEFT JOIN variant_totals vt ON vt.job_id=job.id
)
SELECT recipe_version,
       COUNT(*)::bigint AS current_slot_books,
       COUNT(*) FILTER (WHERE currently_public_and_approved)::bigint AS public_approved_books,
       COUNT(*) FILTER (WHERE is_withdrawn)::bigint AS withdrawn_slots,
       COUNT(*) FILTER (WHERE source_status='verified')::bigint AS verified_source_rows,
       COUNT(*) FILTER (WHERE source_status='retired')::bigint AS retired_source_rows,
       COUNT(*) FILTER (WHERE pending_review)::bigint AS books_with_pending_replacement,
       COUNT(*) FILTER (WHERE COALESCE(variants,0) <> 2)::bigint AS incomplete_variant_pairs,
       COALESCE(SUM(main_bytes),0)::bigint AS recorded_current_main_bytes,
       COALESCE(SUM(card_bytes),0)::bigint AS recorded_current_card_bytes,
       COALESCE(SUM(recorded_source_bytes),0)::bigint AS recorded_current_original_bytes,
       -- Strict metadata candidates only: NOT authorization for automatic
       -- reapproval, in-place overwrite, CDN publication or source use.
       COUNT(*) FILTER (
           WHERE recipe_version='newboo-cover-lossless-webp-v1'
             AND currently_public_and_approved
             AND source_status='verified'
             AND COALESCE(variants,0)=2
             AND main_bytes>0 AND card_bytes>0
             AND NOT pending_review
       )::bigint AS legacy_metadata_candidates_for_manual_review
  FROM attached
 GROUP BY recipe_version
 ORDER BY recipe_version;

-- 2. Historical + current private variant ledger, grouped by recipe. A row
-- may remain after a deletion, so this is *not* a billable S3-byte total.
SELECT job.recipe_version,
       COUNT(*)::bigint AS variant_ledger_rows,
       COALESCE(SUM(variant.size_bytes),0)::bigint AS variant_ledger_bytes,
       COUNT(*) FILTER (WHERE retirement.state='removed')::bigint AS marked_removed_rows,
       COALESCE(SUM(variant.size_bytes) FILTER (
           WHERE retirement.state IS DISTINCT FROM 'removed'
       ),0)::bigint AS not_marked_removed_variant_bytes
  FROM cover_variants variant
  JOIN cover_variant_jobs job ON job.id=variant.job_id
  LEFT JOIN cover_private_variant_removals retirement
    ON retirement.job_id=variant.job_id
   AND retirement.variant_name=variant.variant_name
   AND retirement.storage_key=variant.storage_key
 GROUP BY job.recipe_version
 ORDER BY job.recipe_version;

-- 3. Source originals: status is a *metadata state*, not live S3 proof.
SELECT status AS source_state,
       COUNT(*)::bigint AS source_rows,
       COALESCE(SUM(size_bytes),0)::bigint AS recorded_original_bytes
  FROM cover_upload_objects
 GROUP BY status
 ORDER BY status;

-- 4. CDN public copies: 'copied' is a registry state, not a verified
-- current edge-cache byte count. Retained historical rows are included.
SELECT state AS cdn_publication_state,
       COUNT(*)::bigint AS publication_rows,
       COALESCE(SUM(size_bytes),0)::bigint AS recorded_public_bytes
  FROM public_cover_publications
 GROUP BY state
 ORDER BY state;

ROLLBACK;
