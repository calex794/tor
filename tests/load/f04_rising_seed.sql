-- F-04 query-plan fixture: 100,000 synthetic completions on an existing
-- disposable 100,000-book F-02 catalogue. This does NOT attest C-verification
-- of artificial users: the real C/outbox correctness test lives in f04_rising_pg.
-- Never use on real/retained data. Invocation requires tor_migrator on tor_ci.
DO $$
BEGIN
  IF current_database() <> 'tor_ci' OR current_user <> 'tor_migrator' THEN
    RAISE EXCEPTION 'F04 bulk fixture requires disposable tor_ci as tor_migrator';
  END IF;
  IF (SELECT COUNT(*) FROM books WHERE source_identity LIKE 'ci:f02:100k:%') <> 100000 THEN
    RAISE EXCEPTION 'F04 fixture requires existing F02 100k book seed';
  END IF;
END
$$;

INSERT INTO users(id,email_normalized,display_name,password_hash)
OVERRIDING SYSTEM VALUE
SELECT 9000000+n, 'f04-perf-' || n || '@example.invalid',
       'F04 performance reader ' || n, '$argon2id$fixture'
FROM generate_series(0,5000) n;

WITH selected AS (
  SELECT id FROM books
  WHERE source_identity LIKE 'ci:f02:100k:%'
    AND visibility='public' AND visibility_status='active'
    AND access_level='basic' AND NOT admin_hold AND NOT is_deleted
  ORDER BY id LIMIT 20
)
UPDATE books b SET owner_user_id=9000000
FROM selected s WHERE b.id=s.id;

INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
SELECT id,1,'F04 load free chapter','public','free',0
FROM books WHERE owner_user_id=9000000;

INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
SELECT id,1,E'Начало\n\nКонец😀'
FROM chapters WHERE title='F04 load free chapter';

INSERT INTO chapter_publications(chapter_id,version_id)
SELECT v.chapter_id,v.id FROM chapter_versions v
JOIN chapters c ON c.id=v.chapter_id
WHERE c.title='F04 load free chapter';

-- Bulk-insert valid *shapes* of completion/outbox pairs, all restricted to
-- the disposable workload. Only real PgReadingSync tests prove provenance.
WITH committed AS (
  INSERT INTO reader_chapter_completions(
      user_id,book_id,chapter_id,first_version_id,source_event_id,completed_at)
  SELECT reader.id,c.book_id,c.id,v.id,gen_random_uuid(),
         now() - CASE WHEN reader.id % 3 = 0
                 THEN interval '10 days' ELSE interval '3 days' END
  FROM users reader
  CROSS JOIN chapters c
  JOIN chapter_versions v ON v.chapter_id=c.id AND v.version_no=1
  WHERE reader.id BETWEEN 9000001 AND 9005000
    AND c.title='F04 load free chapter'
  RETURNING user_id,book_id,chapter_id,first_version_id,source_event_id
)
INSERT INTO outbox_events(
  id,event_type,schema_version,aggregate_kind,aggregate_id,
  dedupe_key,payload,required_consumers)
SELECT source_event_id,'ChapterCompletedV1',1,'chapter',chapter_id,
       concat('chapter-completed:',user_id,':',chapter_id),
       jsonb_build_object('user_id',user_id,'book_id',book_id,
         'chapter_id',chapter_id,'version_id',first_version_id,
         'authority','C-verified-free-position'),
       ARRAY['ranking-eligibility-v1','achievements-v1']
FROM committed;

DO $$
BEGIN
  IF (SELECT COUNT(*) FROM reader_chapter_completions
      WHERE user_id BETWEEN 9000001 AND 9005000) <> 100000 THEN
    RAISE EXCEPTION 'F04 fixture incomplete: expected 100k completions';
  END IF;
  IF (SELECT COUNT(*) FROM outbox_events
      WHERE event_type='ChapterCompletedV1'
      AND dedupe_key LIKE 'chapter-completed:900%') <> 100000 THEN
    RAISE EXCEPTION 'F04 fixture incomplete: expected 100k outbox events';
  END IF;
END
$$;

ANALYZE books;
ANALYZE users;
ANALYZE chapters;
ANALYZE chapter_publications;
ANALYZE chapter_versions;
ANALYZE reader_chapter_completions;
ANALYZE outbox_events;
SELECT 'F04_100K_SEEDED' AS event, COUNT(*) AS completions
FROM reader_chapter_completions
WHERE user_id BETWEEN 9000001 AND 9005000;
