//! Real production retirement supervisor, with old installed rows and late C facts.
use super::pg;
use std::time::{Duration, Instant};
use tor_books::repositories::PgCatalog;
use tor_reading::{repositories::PgReadingSync, services::progress::ReadingPlace};

pub(super) fn seed(url: &str, actor: i64, book: i64) {
    pg(
        url,
        &format!(
            "INSERT INTO rec_reader_signals(subject_id,book_id,occurred_at,source_event_id)
           SELECT s.subject_id,c.book_id,c.started_at,c.source_event_id
           FROM rec_subjects s JOIN reader_chapter_starts c ON c.user_id=s.user_id
           WHERE s.user_id={actor} AND c.book_id={book};
         INSERT INTO reader_activity_days(subject_id,book_id,activity_day,first_seen_at)
           SELECT subject_id,{book},((now()-interval '31 days') AT TIME ZONE 'UTC')::date,
             now()-interval '31 days' FROM rec_subjects WHERE user_id={actor};
         WITH coverage AS (
           INSERT INTO reader_return_coverage(started_at,last_heartbeat_at,ended_at)
           VALUES(now()-interval '40 days',now()-interval '39 days',now()-interval '39 days')
           RETURNING id
         ) INSERT INTO reader_return_subjects(subject_id,cohort_day,coverage_epoch_id,
             returned_d1,returned_d7,returned_d30)
           SELECT s.subject_id,(now() AT TIME ZONE 'UTC')::date-31,c.id,false,false,false
           FROM rec_subjects s CROSS JOIN coverage c WHERE s.user_id={actor};
         INSERT INTO reader_book_return_subjects(subject_id,book_id,cohort_day)
           SELECT subject_id,{book},(now() AT TIME ZONE 'UTC')::date-31
           FROM rec_subjects WHERE user_id={actor}"
        ),
    );
    assert!(!expired(url, actor));
}

pub(super) fn expired(url: &str, actor: i64) -> bool {
    pg(url, &format!(
        "SELECT concat_ws('|',
           (SELECT count(*) FROM rec_reader_signals WHERE subject_id=s.subject_id),
           (SELECT count(*) FROM reader_activity_days WHERE subject_id=s.subject_id),
           (SELECT count(*) FROM reader_book_return_subjects WHERE subject_id=s.subject_id),
           (SELECT count(*) FROM reader_return_subjects WHERE subject_id=s.subject_id AND cohort_day IS NOT NULL),
           (SELECT count(*) FROM reader_return_subjects WHERE subject_id=s.subject_id AND cohort_day IS NULL))
         FROM rec_subjects s WHERE user_id={actor}"
    )) == "0|0|0|0|1"
}

pub(super) fn assert_preserved(url: &str, actor: i64, book: i64) {
    assert!(expired(url, actor));
    assert_eq!(
        pg(
            url,
            &format!(
        "SELECT concat_ws('|',count(*),sum(enrolled),sum(eligible),sum(returned),sum(unknown))
         FROM reader_book_return_daily WHERE book_id={book}"
    )
        ),
        "2|2|0|0|2"
    );
    // The old dateless enrollment marker is deliberately visible to this test.
    // Its full retirement is P2b, not a claim made by the runtime-only change.
    assert_eq!(
        pg(
            url,
            "SELECT (SELECT count(*) FROM reader_return_coverage WHERE ended_at IS NULL)
              +(SELECT count(*) FROM reader_book_return_coverage WHERE ended_at IS NULL)"
        ),
        "0"
    );
}

fn canonical(url: &str, actor: i64, chapter: i64) -> String {
    pg(
        url,
        &format!(
            "SELECT json_build_array(c.user_id,c.book_id,c.chapter_id,c.first_version_id,
           c.purchase_id,c.started_at::text,c.source_event_id::text,
           e.event_type,e.schema_version,e.aggregate_kind,e.aggregate_id,e.dedupe_key,
           e.payload,e.created_at::text,e.required_consumers)
         FROM reader_chapter_starts c JOIN outbox_events e ON e.id=c.source_event_id
         WHERE c.user_id={actor} AND c.chapter_id={chapter}"
        ),
    )
}

pub(super) async fn assert_no_new_consumption(url: &str, actor: i64, book: i64) {
    let chapter: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
         VALUES({book},2,'Late canonical chapter','public','free') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES({chapter},1,'abcdefghij') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    let revision: i64 = pg(
        url,
        &format!(
            "SELECT revision FROM reader_book_positions WHERE user_id={actor} AND book_id={book}"
        ),
    )
    .parse()
    .unwrap();
    let catalog = PgCatalog::connect(url).await.unwrap();
    // This direct legacy repository call models an older producer still alive
    // before P2b. It is not the new production HTTP factory or an opt-in path.
    PgReadingSync::new(catalog.pool().clone())
        .with_flow_collection(true)
        .save_free(
            actor,
            chapter,
            version,
            ReadingPlace {
                block_index: 1,
                char_offset: 2,
            },
            revision,
        )
        .await
        .unwrap();
    catalog.pool().close().await;
    // Model a late commit whose canonical occurrence is already beyond the
    // former five-minute attribution grace. No worker may acknowledge it.
    pg(
        url,
        &format!(
            "BEGIN;
         UPDATE reader_chapter_starts SET started_at=now()-interval '8 minutes'
           WHERE user_id={actor} AND chapter_id={chapter};
         UPDATE reader_flow_read_receipts r SET occurred_at=c.started_at
           FROM reader_chapter_starts c
           WHERE c.user_id={actor} AND c.chapter_id={chapter} AND r.event_id=c.source_event_id;
         INSERT INTO flow_book_cohorts(id,subject_id,flow_epoch,visit_id,visit_started_at,
             book_id,first_entry_at,expires_at,acquisition_hint,discovery_kind,has_detail)
           SELECT v.id,v.subject_id,v.flow_epoch,v.id,v.started_at,{book},v.started_at,
             v.started_at+interval '30 days',v.acquisition_hint,3,true
           FROM flow_visits v JOIN rec_subjects s ON s.subject_id=v.subject_id
           WHERE s.user_id={actor} ORDER BY v.started_at DESC LIMIT 1;
         COMMIT"
        ),
    );
    let before = canonical(url, actor, chapter);
    assert!(!before.is_empty());
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM reader_flow_read_receipts r
         JOIN reader_chapter_starts c ON c.source_event_id=r.event_id
         WHERE c.user_id={actor} AND c.chapter_id={chapter}"
            )
        ),
        "1",
        "the late legacy proof is present and eligible by age"
    );
    // The final C cleanup step of the next supervisor pass removes this
    // expired sentinel. Observing its removal proves the preceding F/REC
    // maintenance calls had a chance to consume the now-eligible C fact.
    let sentinel = pg(url, &format!(
        "INSERT INTO reader_flow_book_actions(event_id,subject_id,flow_epoch,book_id,kind,occurred_at)
         SELECT gen_random_uuid(),subject_id,flow_epoch,{book},1,now()-interval '31 days'
         FROM rec_subjects WHERE user_id={actor} RETURNING event_id::text"
    ));
    let checkpoint = pg(url, "SELECT clock_timestamp()::text");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let completed = pg(
            url,
            &format!(
                "SELECT count(*) FROM flow_book_worker_health
             WHERE last_success>'{checkpoint}'::timestamptz"
            ),
        ) == "1";
        let erased = pg(
            url,
            &format!(
                "SELECT count(*) FROM reader_flow_book_actions WHERE event_id='{sentinel}'::uuid"
            ),
        ) == "0";
        if completed && erased {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "retirement supervisor did not complete a pass"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM event_deliveries d
         JOIN reader_chapter_starts c ON c.source_event_id=d.event_id
         WHERE c.user_id={actor} AND c.chapter_id={chapter}
           AND d.consumer IN ('recommendation-feedback-v1','flow-read-attribution-v1')"
            )
        ),
        "0",
        "retired consumers never acknowledge a new canonical C fact"
    );
    assert_eq!(pg(url, &format!(
        "SELECT concat_ws('|',started::int,completed::int,saved::int,followed::int,new_reader::int,d7_state)
         FROM flow_book_cohorts c JOIN rec_subjects s ON s.subject_id=c.subject_id
         WHERE s.user_id={actor} AND c.book_id={book}"
    )), "0|0|0|0|0|0", "late facts do not enrich a retained cohort");
    assert_eq!(canonical(url, actor, chapter), before);
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM reader_book_positions
         WHERE user_id={actor} AND book_id={book} AND chapter_id={chapter}
           AND revision={revision}+1"
            )
        ),
        "1",
        "canonical progress remains committed"
    );
}
