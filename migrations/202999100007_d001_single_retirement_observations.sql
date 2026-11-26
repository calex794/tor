-- D-001: record exact FREE SINGLE retirement observations in PG.
-- This queue is advisory, not a lease or deletion authorization. No rights,
-- immutable receipts, object bytes or active storage placements are mutated.
-- Never consume a stale observation as permission to remove an object.
CREATE VIEW d001_free_single_retirement_preflight AS
SELECT f.chapter_id, f.version_id, f.bucket, f.object_key, f.sha256, f.size_bytes
FROM free_chapter_storage_objects f
JOIN chapter_versions v
  ON v.chapter_id=f.chapter_id AND v.id=f.version_id
JOIN chapters c ON c.id=f.chapter_id
JOIN books b ON b.id=c.book_id
JOIN chapter_publications p ON p.chapter_id=c.id
JOIN chapter_versions head ON head.chapter_id=c.id AND head.id=p.version_id
JOIN d001_chapter_materialization_roles roles
  ON roles.chapter_id=f.chapter_id AND roles.version_id=f.version_id
WHERE NOT roles.is_logically_retained
  AND NOT roles.has_cursor_reference
  AND c.access_level='free' AND c.price_amount=0
  AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
  AND b.visibility='public' AND b.visibility_status='active'
  AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
  AND NOT b.admin_hold AND NOT b.is_deleted
  AND v.version_no<head.version_no
  AND v.body_markdown IS NULL AND v.storage_object_id IS NULL
  AND v.content_sha256=f.sha256
  -- Even expired but not yet cleaned intents block observation: a confirmation
  -- may have passed its earlier expiry check before taking its own locks.
  AND NOT EXISTS (
      SELECT 1 FROM reader_free_open_intents i
      WHERE i.chapter_id=f.chapter_id AND i.version_id=f.version_id
  )
  -- Keep SINGLEs involved in any PACK history/job, including rollback sources.
  AND NOT EXISTS (
      SELECT 1 FROM free_edition_storage_generations g
      WHERE g.chapter_id=f.chapter_id AND g.version_id=f.version_id
        AND g.placement_kind='pack'
  )
  AND NOT EXISTS (
      SELECT 1 FROM free_pack_job_members m
      WHERE m.chapter_id=f.chapter_id AND m.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM free_chapter_import_reservations r
      WHERE r.chapter_id=f.chapter_id AND r.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM chapter_scheduled_releases s
      WHERE s.chapter_id=f.chapter_id AND s.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM social_free_comment_reports r
      WHERE r.chapter_id=f.chapter_id AND r.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM social_paid_comment_reports r
      WHERE r.chapter_id=f.chapter_id AND r.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM finance_direct_swap_commands s
      WHERE s.target_chapter_id=f.chapter_id AND s.target_version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM chapter_encrypted_release_events e
      WHERE e.chapter_id=f.chapter_id AND e.version_id=f.version_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM chapter_version_assets a
      WHERE a.chapter_id=f.chapter_id AND a.version_id=f.version_id
  );

COMMENT ON VIEW d001_free_single_retirement_preflight IS
    'Conservative snapshot of superseded FREE SINGLE receipts. Unknown/soft references, admin history, grant issuance, actual S3 bytes and independent DR still require separate guards. NOT a DELETE permit.';

CREATE TABLE d001_free_single_retirement_observations (
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    first_observed_at TIMESTAMPTZ NOT NULL,
    last_observed_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY(chapter_id,version_id),
    FOREIGN KEY(chapter_id,version_id)
      REFERENCES free_chapter_storage_objects(chapter_id,version_id)
      ON DELETE RESTRICT,
    CHECK (last_observed_at>=first_observed_at)
);

COMMENT ON TABLE d001_free_single_retirement_observations IS
    'Durable observation history referencing immutable exact SINGLE receipts. Timestamps are observations, not proof of uninterrupted eligibility or elapsed grant lifetime.';

-- Bounded, chapter-local, idempotent observation. No storage credentials,
-- no dynamic SQL, no elevated privileges. Do not schedule this automatically.
CREATE FUNCTION d001_record_free_single_retirement_candidates(
    p_chapter_id BIGINT, p_after_version_id BIGINT, p_limit INTEGER
) RETURNS SETOF d001_free_single_retirement_observations
LANGUAGE plpgsql SECURITY INVOKER AS $$
BEGIN
    IF p_chapter_id IS NULL OR p_chapter_id<=0
        OR p_after_version_id IS NULL OR p_after_version_id<0
        OR p_limit IS NULL OR p_limit NOT BETWEEN 1 AND 128
    THEN
        RAISE EXCEPTION 'invalid D001 retirement observation page'
            USING ERRCODE='22023';
    END IF;

    RETURN QUERY
    INSERT INTO d001_free_single_retirement_observations AS recorded
        (chapter_id,version_id,first_observed_at,last_observed_at)
    SELECT f.chapter_id,f.version_id,statement_timestamp(),statement_timestamp()
      FROM d001_free_single_retirement_preflight f
     WHERE f.chapter_id=p_chapter_id AND f.version_id>p_after_version_id
     ORDER BY f.version_id LIMIT p_limit
    ON CONFLICT(chapter_id,version_id) DO UPDATE
        SET last_observed_at=GREATEST(
            recorded.last_observed_at,EXCLUDED.last_observed_at)
    RETURNING recorded.*;
END;
$$;

-- Never persist a stale eligibility flag. A new pin, intent, PACK reference,
-- hold, or changed head is visible immediately on the next statement.
CREATE VIEW d001_free_single_retirement_queue AS
SELECT o.chapter_id,o.version_id,f.bucket,f.object_key,f.sha256,f.size_bytes,
       o.first_observed_at,o.last_observed_at,
       fresh.version_id IS NOT NULL AS candidate_now
FROM d001_free_single_retirement_observations o
JOIN free_chapter_storage_objects f USING(chapter_id,version_id)
LEFT JOIN d001_free_single_retirement_preflight fresh USING(chapter_id,version_id);

COMMENT ON VIEW d001_free_single_retirement_queue IS
    'Recorded exact receipts plus current advisory candidate status. No execution path to S3 DELETE; eligibility is not guaranteed after the SQL statement ends.';
