//! Only the real AUTH=0 entry point may perform these expiry operations.
use super::*;
use tor_reading::{
    repositories::{PgLibrary, PgReadingSync},
    services::progress::ReadingPlace,
};

pub(super) async fn seed(url: &str, actor: i64) -> i64 {
    let book: i64 = pg(url, &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES('ci:flow-startup:{actor}','ru','FLOW cleanup fixture','public','basic') RETURNING id"
    )).parse().unwrap();
    let chapter: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
         VALUES({book},1,'Cleanup chapter','public','free') RETURNING id"
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
    let catalog = PgCatalog::connect(url).await.unwrap();
    let pool = catalog.pool();
    PgLibrary::new(pool.clone())
        .with_flow_collection(true)
        .add_book(actor, book)
        .await
        .unwrap();
    PgReadingSync::new(pool.clone())
        .with_flow_collection(true)
        .save_free(
            actor,
            chapter,
            version,
            ReadingPlace {
                block_index: 1,
                char_offset: 2,
            },
            0,
        )
        .await
        .unwrap();
    pool.close().await;
    // Actual C writes above establish business invariants. Only this isolated
    // fixture moves their timestamps past retention, and seeds an expired F
    // projection to exercise production cleanup with collection disabled.
    pg(url, &format!(
        "BEGIN;
         UPDATE reader_chapter_starts SET started_at=started_at-interval '31 days' WHERE user_id={actor};
         UPDATE reader_flow_read_receipts SET occurred_at=occurred_at-interval '31 days'
           WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id={actor});
         UPDATE reader_flow_book_actions SET occurred_at=occurred_at-interval '31 days'
           WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id={actor});
         UPDATE reader_flow_activity_days SET first_seen_at=first_seen_at-interval '31 days',
           activity_day=activity_day-31
           WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id={actor});
         INSERT INTO event_deliveries(consumer,event_id)
           SELECT mode,source_event_id FROM reader_chapter_starts
             CROSS JOIN (VALUES('flow-read-attribution-v1'),('flow-startup-preserved')) AS modes(mode)
           WHERE user_id={actor};
         INSERT INTO flow_book_action_receipts(event_id,subject_id,flow_epoch,occurred_at)
           SELECT event_id,subject_id,flow_epoch,occurred_at FROM reader_flow_book_actions
           WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id={actor});
         INSERT INTO flow_book_cohorts(id,subject_id,flow_epoch,visit_id,visit_started_at,book_id,
           first_entry_at,expires_at,acquisition_hint,discovery_kind,has_detail,started,saved,
           first_start_at,first_start_event_id,saved_event_id)
           SELECT v.id,v.subject_id,v.flow_epoch,v.id,v.started_at,{book},v.started_at,
             v.started_at+interval '30 days',v.acquisition_hint,3,true,true,true,
             c.started_at,c.source_event_id,a.event_id
           FROM flow_visits v JOIN rec_subjects s ON s.subject_id=v.subject_id
           JOIN reader_chapter_starts c ON c.user_id=s.user_id AND c.book_id={book}
           JOIN reader_flow_book_actions a ON a.subject_id=s.subject_id AND a.book_id={book}
           WHERE s.user_id={actor} AND v.started_at<now()-interval '30 days';
         COMMIT;"
    ));
    assert!(
        !expired(url, actor),
        "expired fixture must exist before the real server starts"
    );
    book
}

pub(super) fn expired(url: &str, actor: i64) -> bool {
    pg(url, &format!(
        "SELECT concat_ws('|',
         (SELECT count(*) FROM flow_book_cohorts c WHERE c.subject_id=s.subject_id),
         (SELECT count(*) FROM flow_book_action_receipts c WHERE c.subject_id=s.subject_id),
         (SELECT count(*) FROM reader_flow_book_actions c WHERE c.subject_id=s.subject_id),
         (SELECT count(*) FROM reader_flow_activity_days c WHERE c.subject_id=s.subject_id),
         (SELECT count(*) FROM reader_flow_read_receipts c WHERE c.subject_id=s.subject_id),
         (SELECT count(*) FROM event_deliveries d JOIN reader_chapter_starts c ON c.source_event_id=d.event_id
           WHERE c.user_id={actor} AND d.consumer='flow-read-attribution-v1'))
         FROM rec_subjects s WHERE s.user_id={actor}"
    )) == "0|0|0|0|0|0"
}

pub(super) fn assert_preserved(url: &str, actor: i64, book: i64) {
    assert_eq!(pg(url, &format!(
        "SELECT concat_ws('|',
         (SELECT count(*) FROM reader_chapter_starts WHERE user_id={actor}),
         (SELECT count(*) FROM reader_book_positions WHERE user_id={actor}),
         (SELECT count(*) FROM reader_library_items WHERE user_id={actor}),
         (SELECT count(*) FROM event_deliveries d JOIN reader_chapter_starts c ON c.source_event_id=d.event_id
           WHERE c.user_id={actor} AND d.consumer='flow-startup-preserved'),
         (SELECT sum(cohorts) FROM flow_book_daily WHERE book_id={book}),
         (SELECT sum(starts) FROM flow_book_daily WHERE book_id={book}),
         (SELECT sum(saves) FROM flow_book_daily WHERE book_id={book}))"
    )), "1|1|1|1|1|1|1", "cleanup preserves business facts and anonymous totals");
}
