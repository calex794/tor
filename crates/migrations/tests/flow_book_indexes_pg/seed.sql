-- Synthetic canonical facts in an explicitly disposable test database.
DO $fixture$
DECLARE
    v_account bigint;
    v_book bigint;
    v_chapter bigint;
    v_version bigint;
    v_started uuid;
    v_completed uuid;
BEGIN
    INSERT INTO users(email_normalized,display_name,password_hash)
        VALUES('flow-index@example.invalid','FLOW index fixture','$argon2id$ci-placeholder')
        RETURNING id INTO v_account;
    INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
        VALUES('flow-index-book','ru','FLOW index fixture','public',v_account,'basic')
        RETURNING id INTO v_book;
    INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,reader_released_at)
        VALUES(v_book,1,'Index fixture','public','free',now())
        RETURNING id INTO v_chapter;
    INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
        VALUES(v_chapter,1,'Synthetic test chapter')
        RETURNING id INTO v_version;
    INSERT INTO chapter_publications(chapter_id,version_id)
        VALUES(v_chapter,v_version);
    INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,aggregate_id,
        dedupe_key,payload,required_consumers)
        VALUES('ChapterStartedV1',1,'chapter',v_chapter,'flow-index:start',
            jsonb_build_object('user_id',v_account::text,'book_id',v_book::text,
                'chapter_id',v_chapter::text,'version_id',v_version::text,
                'authority','C-verified-free-start'),
            ARRAY['recommendation-feedback-v1','reading-rankings-v1'])
        RETURNING id INTO v_started;
    INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,aggregate_id,
        dedupe_key,payload,required_consumers)
        VALUES('ChapterCompletedV1',1,'chapter',v_chapter,'flow-index:completed',
            jsonb_build_object('user_id',v_account::text,'book_id',v_book::text,
                'chapter_id',v_chapter::text,'version_id',v_version::text,
                'authority','C-verified-free-position'),
            ARRAY['recommendation-feedback-v1','reading-rankings-v1'])
        RETURNING id INTO v_completed;
    INSERT INTO reader_chapter_starts(user_id,book_id,chapter_id,first_version_id,
        started_at,source_event_id)
        VALUES(v_account,v_book,v_chapter,v_version,now()-interval '2 days',v_started);
    INSERT INTO reader_chapter_completions(user_id,book_id,chapter_id,first_version_id,
        completed_at,source_event_id)
        VALUES(v_account,v_book,v_chapter,v_version,now()-interval '1 day',v_completed);
END
$fixture$;
