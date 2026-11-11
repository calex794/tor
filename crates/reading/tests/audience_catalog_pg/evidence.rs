use crate::fixture::Fixture;
use tor_reading::{
    repositories::audience::PgAudience,
    services::audience::{
        canonical::{CanonicalDimension, CanonicalQuery},
        AudienceError, ClosedChapterCounts,
    },
};

async fn fixture() -> Fixture {
    let f = Fixture::new().await;
    f.sql("CREATE TABLE reader_chapter_starts (LIKE public.reader_chapter_starts INCLUDING ALL);
        REVOKE ALL ON reader_chapter_starts FROM tor_app;
        GRANT SELECT ON reader_chapter_starts TO tor_app").await;
    f.books(8).await;
    f
}

fn at(anchor: i64, seconds: i64) -> String {
    format!("to_timestamp(({}::bigint)::double precision)", anchor+seconds)
}

async fn event(f: &Fixture, book: i64, user: i64, chapter: i64, time: &str, finish: bool, mode: &str) {
    assert!(["free","paid","invalid","mismatch","missing"].contains(&mode));
    let (table, column, kind, free, paid) = if finish {
        ("reader_chapter_completions","completed_at","ChapterCompletedV1","C-verified-free-position","C-verified-purchased-position")
    } else {
        ("reader_chapter_starts","started_at","ChapterStartedV1","C-verified-free-start","C-verified-purchased-start")
    };
    let purchase = if matches!(mode,"paid"|"mismatch") { "123::bigint" } else { "NULL::bigint" };
    let authority = if mode=="invalid" { "untrusted" } else if matches!(mode,"paid"|"mismatch") { paid } else { free };
    let extra = match mode {
        "paid"=>"||jsonb_build_object('purchase_id',123)",
        "mismatch"=>"||jsonb_build_object('purchase_id',999)",
        _=>"",
    };
    f.sql(&format!("WITH facts AS (
        INSERT INTO {table}(user_id,book_id,chapter_id,first_version_id,{column},source_event_id,purchase_id)
        VALUES({user},{book},{chapter},{chapter},{time},gen_random_uuid(),{purchase})
        ON CONFLICT(user_id,book_id,chapter_id) DO NOTHING RETURNING *)
        INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
        SELECT source_event_id,'{kind}',1,'chapter',chapter_id,source_event_id::text,
          jsonb_build_object('user_id',user_id,'book_id',book_id,'chapter_id',chapter_id,
          'version_id',first_version_id,'authority','{authority}'){extra},ARRAY['fixture-only']
        FROM facts WHERE {}",mode!="missing")).await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn established_reader_new_chapters_are_present_and_missing_proofs_are_never_zero() {
    let f = fixture().await;
    let store = PgAudience::new(f.app.clone());
    let anchor = store.closed_report(1).await.unwrap().until_epoch;
    event(&f,1,1,11,&at(anchor,-200*86400),true,"free").await;
    for chapter in [12,13] {
        event(&f,1,1,chapter,&at(anchor,-7200),true,"free").await;
    }
    event(&f,1,1,12,&at(anchor,-10800),false,"free").await;
    // Repeating the same canonical chapter request adds no new row or event.
    event(&f,1,1,12,&at(anchor,-3600),true,"free").await;
    event(&f,1,1,12,&at(anchor,-3600),false,"free").await;
    event(&f,1,2,21,&at(anchor,-90000),true,"paid").await;
    event(&f,1,2,21,&at(anchor,-93600),false,"paid").await;
    event(&f,3,3,31,&at(anchor,-7200),true,"missing").await;
    event(&f,3,4,32,&at(anchor,-7200),true,"free").await;
    event(&f,4,4,41,&at(anchor,-90000),false,"missing").await;
    event(&f,5,5,51,&at(anchor,-7200),true,"mismatch").await;
    event(&f,6,6,61,&at(anchor,-7200),false,"invalid").await;
    // Unmatched evidence outside both selected windows cannot poison them.
    event(&f,7,7,71,&at(anchor,3600),true,"missing").await;
    event(&f,7,7,72,&at(anchor,-181*86400),false,"invalid").await;
    event(&f,8,8,81,&at(anchor,-7200),true,"free").await;
    f.sql("DELETE FROM reader_return_public_books WHERE id=8").await;
    let report = store.closed_book_evidence(1,anchor,&[999,8,7,6,5,4,3,2,1,1]).await.unwrap();
    assert_eq!(report.until_epoch,anchor);
    assert_eq!(report.days,1);
    assert_eq!(report.rows.iter().map(|row|row.book_id).collect::<Vec<_>>(),[1,2,3,4,5,6,7,8,999]);
    assert_eq!(report.rows[0].current,Some(ClosedChapterCounts{starts:1,chapter_completions:2}));
    assert_eq!(report.rows[0].previous,Some(ClosedChapterCounts{starts:1,chapter_completions:1}));
    assert_eq!(report.rows[1].current,Some(ClosedChapterCounts::default()));
    assert_eq!(report.rows[1].previous,Some(ClosedChapterCounts::default()));
    assert_eq!(report.rows[2].current,None);
    assert_eq!(report.rows[2].previous,Some(ClosedChapterCounts::default()));
    assert_eq!(report.rows[3].current,Some(ClosedChapterCounts::default()));
    assert_eq!(report.rows[3].previous,None);
    assert_eq!(report.rows[4].current,None);
    assert_eq!(report.rows[5].current,None);
    assert_eq!(report.rows[6].current,Some(ClosedChapterCounts::default()));
    assert_eq!(report.rows[6].previous,Some(ClosedChapterCounts::default()));
    for row in &report.rows[7..] {
        assert_eq!(row.current,None);
        assert_eq!(row.previous,None);
    }
    let canonical = store.canonical_report(&CanonicalQuery::new(1,CanonicalDimension::Books)).await.unwrap();
    let existing = canonical.rows.iter().find(|row|row.book_id==Some(1)).unwrap();
    assert_eq!(existing.counts.current_pairs,0,"the first-book metric would miss this established reader");
    for private_key in ["user_id","source_event_id","purchase_id","chapter_id","version_id"] {
        assert!(!format!("{report:?}").contains(private_key));
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn exact_closed_windows_retain_old_evidence_and_reject_unbounded_or_stale_input() {
    let f = fixture().await;
    let store = PgAudience::new(f.app.clone());
    let anchor = store.closed_report(1).await.unwrap().until_epoch;
    for (index,days) in [1,7,30,90].into_iter().enumerate() {
        let book = (index+1) as i64;
        let span = i64::from(days)*86400;
        for (offset,seconds) in [-span*2-1,-span*2,-span,-1,0].into_iter().enumerate() {
            let chapter = book*100+offset as i64;
            for finish in [false,true] {
                event(&f,book,chapter,chapter,&at(anchor,seconds),finish,"free").await;
            }
        }
        let report = store.closed_book_evidence(days,anchor,&[book]).await.unwrap();
        assert_eq!(report.rows[0].current,Some(ClosedChapterCounts{starts:2,chapter_completions:2}));
        assert_eq!(report.rows[0].previous,Some(ClosedChapterCounts{starts:1,chapter_completions:1}));
    }
    let prior = store.closed_book_evidence(7,anchor-3600,&[5]).await.unwrap();
    assert_eq!(prior.until_epoch,anchor-3600);
    assert_eq!(prior.rows[0].current,Some(ClosedChapterCounts::default()));
    assert!(store.closed_book_evidence(1,anchor,&[]).await.unwrap().rows.is_empty());
    assert_eq!(store.closed_book_evidence(1,anchor,&[1;50]).await.unwrap().rows.len(),1);
    for ids in [vec![1;51],vec![0],vec![-1]] {
        assert_eq!(store.closed_book_evidence(1,anchor,&ids).await,Err(AudienceError::Invalid));
    }
    for days in [0,2,91] {
        assert_eq!(store.closed_book_evidence(days,anchor,&[1]).await,Err(AudienceError::Invalid));
    }
    for until in [0,anchor-1,anchor+1,anchor+3600,anchor-7200,i64::MAX-i64::MAX%3600] {
        assert_eq!(store.closed_book_evidence(1,until,&[1]).await,Err(AudienceError::Invalid));
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn unavailable_sources_and_select_only_role_never_become_empty_evidence() {
    let f = fixture().await;
    let store = PgAudience::new(f.app.clone());
    let anchor = store.closed_report(1).await.unwrap().until_epoch;
    for table in ["reader_return_public_books","reader_chapter_starts","reader_chapter_completions","outbox_events"] {
        let mut tx = f.owner.begin().await.unwrap();
        sqlx::query(&format!("LOCK TABLE {}.{table} IN ACCESS EXCLUSIVE MODE",f.schema))
            .execute(&mut *tx).await.unwrap();
        let began = std::time::Instant::now();
        assert_eq!(store.closed_book_evidence(1,anchor,&[1]).await,Err(AudienceError::Unavailable));
        assert!(began.elapsed()<std::time::Duration::from_secs(1));
        tx.rollback().await.unwrap();
    }
    f.sql("REVOKE SELECT ON reader_chapter_starts FROM tor_app").await;
    assert_eq!(store.closed_book_evidence(1,anchor,&[1]).await,Err(AudienceError::Unavailable));
    f.sql("GRANT SELECT ON reader_chapter_starts TO tor_app").await;
    let recovered = store.closed_book_evidence(1,anchor,&[1]).await.unwrap();
    assert_eq!(recovered.rows[0].current,Some(ClosedChapterCounts::default()));
    assert!(sqlx::query("DELETE FROM reader_chapter_starts").execute(&f.app).await.is_err());
    f.finish().await;
}
