-- F-04: synthetic growth shelf fixture, only for the isolated tor_ci database.
DO $$
BEGIN
  IF current_database() <> 'tor_ci' OR current_user <> 'tor_app' THEN
    RAISE EXCEPTION 'F04 sandbox must use the isolated tor_ci application role';
  END IF;
END $$;

INSERT INTO users (id,email_normalized,display_name,password_hash)
OVERRIDING SYSTEM VALUE VALUES
  (987600,'f04-sandbox-owner@example.invalid','F04 sandbox owner','$argon2id$fixture'),
  (987601,'f04-sandbox-a@example.invalid','F04 sandbox reader A','$argon2id$fixture'),
  (987602,'f04-sandbox-b@example.invalid','F04 sandbox reader B','$argon2id$fixture'),
  (987603,'f04-sandbox-c@example.invalid','F04 sandbox reader C','$argon2id$fixture');

INSERT INTO books (id,owner_user_id,source_identity,target_language,title,
                   author_name,visibility,access_level)
VALUES (987610,987600,'ci:f04:sandbox:book','ru',
        'F04 Sandbox Rising Book','F04 Public Author','public','basic');

INSERT INTO chapters (id,book_id,chapter_no,title,visibility,access_level,price_amount)
VALUES (987620,987610,1,'F04 Sandbox Free Chapter','public','free',0);

INSERT INTO chapter_versions (id,chapter_id,version_no,body_markdown)
VALUES (987630,987620,1,E'Начало\n\nТекст для проверочного чтения.');

INSERT INTO chapter_publications (chapter_id,version_id)
VALUES (987620,987630);

-- This is a fixture with authoritative-shaped pairs, not a claim that
-- synthetic completions were produced by a genuine reader session.
WITH completions AS (
  INSERT INTO reader_chapter_completions
    (user_id,book_id,chapter_id,first_version_id,source_event_id)
  SELECT user_id,987610,987620,987630,gen_random_uuid()
  FROM (VALUES (987601::bigint),(987602::bigint),(987603::bigint)) AS readers(user_id)
  RETURNING user_id,book_id,chapter_id,first_version_id,source_event_id
)
INSERT INTO outbox_events
  (id,event_type,schema_version,aggregate_kind,aggregate_id,
   dedupe_key,payload,required_consumers)
SELECT source_event_id,'ChapterCompletedV1',1,'chapter',chapter_id,
       concat('chapter-completed:',user_id,':',chapter_id),
       jsonb_build_object('user_id',user_id,'book_id',book_id,
                          'chapter_id',chapter_id,'version_id',first_version_id,
                          'authority','C-verified-free-position'),
       ARRAY['ranking-eligibility-v1','achievements-v1']
FROM completions;

DO $$
BEGIN
  IF (SELECT COUNT(*) FROM reader_chapter_completions WHERE book_id=987610) <> 3
     OR (SELECT COUNT(*) FROM outbox_events
         WHERE event_type='ChapterCompletedV1' AND aggregate_id=987620) <> 3 THEN
    RAISE EXCEPTION 'F04 sandbox must have exactly 3 paired reader facts';
  END IF;
END $$;
