-- MOD-COVER-OPS-01 / #667: observation only, NOT an authorization to delete.
-- No writes, no S3 API calls, no DDL, no key/PII output.
\set ON_ERROR_STOP on
\pset pager off
BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY;
SET LOCAL statement_timeout = '10s';
SET LOCAL lock_timeout = '1s';

\echo 'MOD-COVER GC preview: PRIVATE variant counts; 12 internal references per reason'
WITH variants AS (
    SELECT v.job_id, v.variant_name, r.id AS review_id,
           CASE
               WHEN c.state='removed' AND slot.book_id IS NOT NULL
                   THEN 'ANOMALY_removed_while_attached'
               WHEN slot.book_id IS NOT NULL THEN 'retain_attached_slot'
               WHEN c.state='removed' THEN 'already_removed'
               WHEN j.status NOT IN ('done','superseded','dead')
                   THEN 'retain_nonterminal_job'
               WHEN r.id IS NULL THEN 'retain_missing_review'
               -- SQL NOT(NULL) is NULL, not TRUE. Explicit IS NOT TRUE
               -- prevents legacy superseded_at=NULL from leaking into the
               -- eligible_to_stage branch (worker correctly keeps it).
               WHEN (
                   (r.state='rejected' AND r.decided_at<=now()-interval '7 days')
                   OR (r.state='superseded' AND r.superseded_at<=now()-interval '7 days')
               ) IS NOT TRUE THEN 'retain_pending_recent_or_undated_review'
               WHEN c.id IS NULL THEN 'eligible_to_stage'
               WHEN c.state='leased' AND c.locked_until>=now()
                   THEN 'retain_live_lease'
               WHEN c.state='pending' AND c.retry_after>now()
                   THEN 'retain_retry_backoff'
               WHEN (c.state='pending' AND c.retry_after<=now())
                 OR (c.state='leased' AND c.locked_until<now())
                   THEN 'eligible_to_claim'
               ELSE 'retain_ambiguous_ledger'
           END AS disposition
    FROM cover_variants v
    JOIN cover_variant_jobs j ON j.id=v.job_id
    LEFT JOIN book_cover_review_requests r ON r.job_id=j.id
    LEFT JOIN book_cover_slots slot ON slot.job_id=j.id
    LEFT JOIN cover_private_variant_removals c
        ON c.job_id=v.job_id
       AND c.variant_name=v.variant_name
       AND c.storage_key=v.storage_key
)
SELECT 'private_variants' AS asset, disposition, count(*) AS total,
       (array_agg(format('job=%s variant=%s review=%s',
                         job_id,variant_name,coalesce(review_id::text,'none'))
                  ORDER BY job_id,variant_name))[1:12] AS internal_examples
FROM variants
GROUP BY disposition
ORDER BY disposition;

\echo 'MOD-COVER GC preview: shared PRIVATE source counts; 12 internal references per reason'
WITH source_graph AS (
    SELECT src.id AS source_id, src.status AS source_state,
           src.storage_key LIKE 'staging/covers/sources/%' AS key_is_private_source,
           c.id AS removal_id, c.state AS removal_state,
           c.retry_after, c.locked_until,
           EXISTS (
               SELECT 1 FROM cover_variant_jobs j
               WHERE j.source_object_id=src.id
           ) AS has_jobs,
           EXISTS (
               SELECT 1 FROM cover_variant_jobs j
               WHERE j.source_object_id=src.id
                 AND (
                   j.status NOT IN ('done','superseded','dead')
                   OR NOT EXISTS (
                       SELECT 1 FROM book_cover_review_requests r
                       WHERE r.job_id=j.id
                         AND (
                           (r.state='rejected' AND r.decided_at<=now()-interval '7 days')
                           OR (r.state='superseded' AND r.superseded_at<=now()-interval '7 days')
                         )
                   )
                 )
           ) AS has_blocking_job,
           EXISTS (
               SELECT 1 FROM book_cover_slots slot
               JOIN cover_variant_jobs j ON j.id=slot.job_id
               WHERE j.source_object_id=src.id
           ) AS has_attached_slot,
           EXISTS (
               SELECT 1 FROM cover_variant_jobs j
               JOIN cover_variants v ON v.job_id=j.id
               LEFT JOIN cover_private_variant_removals done
                 ON done.job_id=v.job_id
                AND done.variant_name=v.variant_name
                AND done.storage_key=v.storage_key
               WHERE j.source_object_id=src.id
                 AND (done.id IS NULL OR done.state<>'removed')
           ) AS has_unremoved_variants
    FROM cover_upload_objects src
    LEFT JOIN cover_private_source_removals c
      ON c.source_object_id=src.id AND c.storage_key=src.storage_key
),
source_dispositions AS (
    SELECT source_id,
           CASE
               WHEN removal_state='removed' AND
                    (has_attached_slot OR has_blocking_job OR has_unremoved_variants)
                   THEN 'ANOMALY_removed_with_live_reference'
               WHEN has_attached_slot THEN 'retain_attached_slot'
               WHEN NOT key_is_private_source THEN 'retain_non_source_key'
               WHEN source_state NOT IN ('verified','retired')
                   THEN 'retain_unverified_source'
               WHEN NOT has_jobs THEN 'retain_no_jobs_legacy_or_orphan'
               WHEN has_blocking_job THEN 'retain_nonterminal_or_recent_review'
               WHEN has_unremoved_variants THEN 'retain_unremoved_variants'
               WHEN source_state='verified' AND removal_id IS NOT NULL
                   THEN 'ANOMALY_verified_with_removal_ledger'
               WHEN source_state='verified' THEN 'eligible_to_stage'
               WHEN removal_id IS NULL THEN 'ANOMALY_retired_without_removal_ledger'
               WHEN removal_state='removed' THEN 'already_removed'
               WHEN removal_state='leased' AND locked_until>=now()
                   THEN 'retain_live_lease'
               WHEN removal_state='pending' AND retry_after>now()
                   THEN 'retain_retry_backoff'
               WHEN (removal_state='pending' AND retry_after<=now())
                 OR (removal_state='leased' AND locked_until<now())
                   THEN 'eligible_to_claim'
               ELSE 'retain_ambiguous_ledger'
           END AS disposition
    FROM source_graph
)
SELECT 'private_sources' AS asset, disposition, count(*) AS total,
       (array_agg(source_id ORDER BY source_id))[1:12] AS internal_examples
FROM source_dispositions
GROUP BY disposition
ORDER BY disposition;

ROLLBACK;
