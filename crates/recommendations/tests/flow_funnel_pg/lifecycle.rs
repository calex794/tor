use super::*;
use std::time::Duration;

async fn started(f: &Fixture) -> (Book, String) {
    let book = f.book().await;
    f.enter(book, 0, 240, 0).await;
    let start = f.canonical(book, 0, 180, true).await;
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    assert_eq!(f.counts().await.d7_pending, 1);
    (book, start)
}

async fn return_day(f: &Fixture, book: i64, day_offset: i32) {
    sqlx::query(
        "INSERT INTO reader_flow_activity_days(subject_id,flow_epoch,book_id,activity_day,first_seen_at)
         SELECT subject_id,flow_epoch,$1,(started_at AT TIME ZONE 'UTC')::date+$2::int,
           (((started_at AT TIME ZONE 'UTC')::date+$2::int)::timestamp+interval '12 hours') AT TIME ZONE 'UTC'
         FROM flow_book_return_anchors ON CONFLICT DO NOTHING",
    ).bind(book).bind(day_offset).execute(&f.app).await.unwrap();
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn d7_is_positive_book_specific_and_missing_evidence_stays_unknown() {
    let f = Fixture::new("returns").await;
    let (one, _) = started(&f).await;
    let another = f.book().await;
    f.age(8 * 86_400).await;
    return_day(&f, another.id, 7).await;
    return_day(&f, one.id, 6).await;
    f.funnel.cycle(false).await.unwrap();
    let absent = f.counts().await;
    assert_eq!(
        (absent.d7_pending, absent.d7_returned, absent.d7_unknown),
        (0, 0, 1)
    );
    return_day(&f, one.id, 7).await;
    f.funnel.cycle(false).await.unwrap();
    assert_eq!(
        (f.counts().await.d7_returned, f.counts().await.d7_unknown),
        (1, 0)
    );
    f.funnel.cycle(true).await.unwrap();
    assert_eq!(f.counts().await.d7_anchors, 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn visit_origin_cutoff_and_delayed_maintenance_do_not_attribute_a_late_return() {
    let f = Fixture::new("early_expiry").await;
    let (one, _) = started(&f).await;
    // A book can first appear near the end of a genuinely continuous visit.
    // Preserve only one day of this origin's remaining personal retention.
    sqlx::query(
        "UPDATE flow_book_cohorts SET visit_started_at=first_entry_at-interval '29 days',
           expires_at=first_entry_at+interval '1 day'",
    )
    .execute(&f.app)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE flow_book_return_anchors a SET expires_at=c.expires_at FROM flow_book_cohorts c WHERE a.cohort_id=c.id",
    ).execute(&f.app).await.unwrap();
    f.age(8 * 86_400).await;
    return_day(&f, one.id, 7).await;
    f.funnel.cycle(false).await.unwrap();
    let counts = f.counts().await;
    assert_eq!(
        (
            counts.cohorts,
            counts.starts,
            counts.d7_returned,
            counts.d7_unknown
        ),
        (1, 1, 0, 1)
    );
    assert_eq!(f.scalar("SELECT count(*) FROM flow_book_cohorts").await, 0);
    assert_eq!(
        f.scalar("SELECT count(*) FROM flow_book_return_anchors")
            .await,
        0
    );
    f.funnel.cycle(false).await.unwrap();
    assert_eq!(
        f.counts().await,
        counts,
        "finalize/delete retry is exactly once"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn anonymous_retention_and_canonical_first_anchor_never_roll_forward() {
    let f = Fixture::new("anonymous").await;
    let (one, _) = started(&f).await;
    f.age(31 * 86_400).await;
    f.visits.maintain().await.unwrap();
    f.funnel.cycle(false).await.unwrap();
    assert_eq!(f.scalar("SELECT count(*) FROM flow_book_cohorts").await, 0);
    assert_eq!(
        f.scalar("SELECT count(*) FROM flow_book_return_anchors")
            .await,
        0
    );
    assert_eq!(f.deliveries(Some("flow-read-attribution-v1")).await, 0);
    let archived = f.counts().await;
    assert_eq!(
        (archived.cohorts, archived.starts, archived.d7_unknown),
        (1, 1, 1)
    );
    let later_chapter = f.chapter(one.id).await;
    f.enter(one, 1, 240, 1).await;
    f.canonical(later_chapter, 0, 180, true).await;
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    assert_eq!(
        f.scalar("SELECT count(*) FROM flow_book_return_anchors")
            .await,
        0
    );
    assert_eq!(f.counts().await.starts, 2);
    assert_eq!(
        f.counts().await.d7_anchors,
        1,
        "only anonymous old anchor remains"
    );
    f.disable().await;
    assert_eq!(
        f.counts().await,
        archived,
        "withdrawal retracts only retained inputs"
    );
    sqlx::query("UPDATE flow_book_daily SET cohort_day=(now() AT TIME ZONE 'UTC')::date-181")
        .execute(&f.app)
        .await
        .unwrap();
    f.funnel.cycle(false).await.unwrap();
    assert!(f.funnel.report(90).await.unwrap().current.counts.is_none());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn revoke_retracts_retained_finalized_contribution_and_only_own_delivery() {
    let f = Fixture::new("revoke_daily").await;
    let (_, event) = started(&f).await;
    sqlx::query(
        "INSERT INTO event_deliveries(consumer,event_id) VALUES('other-consumer-v1',$1::uuid)",
    )
    .bind(&event)
    .execute(&f.app)
    .await
    .unwrap();
    // A retained finalized recovery row must be retractable without changing
    // unrelated consumers or guessing the anonymous old subject contribution.
    let mut tx = f.app.begin().await.unwrap();
    sqlx::query("UPDATE flow_book_cohorts SET d7_state=3,finalized_at=now()")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO flow_book_daily(cohort_day,book_id,acquisition_hint,discovery_kind,
          cohorts,detail_opens,reader_opens,starts,completions,saves,follows,new_readers,d7_anchors,d7_returned,d7_unknown)
         SELECT (first_entry_at AT TIME ZONE 'UTC')::date,book_id,acquisition_hint,discovery_kind,
           1,has_detail::int,has_reader::int,started::int,completed::int,saved::int,followed::int,new_reader::int,1,0,1
         FROM flow_book_cohorts",
    ).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    f.disable().await;
    assert_eq!(f.scalar("SELECT count(*) FROM flow_book_cohorts").await, 0);
    assert_eq!(
        f.scalar("SELECT count(*) FROM flow_book_return_anchors")
            .await,
        0
    );
    assert_eq!(
        f.scalar(
            "SELECT count(*) FROM flow_book_daily WHERE cohorts=0 AND starts=0 AND d7_anchors=0"
        )
        .await,
        1
    );
    assert_eq!(f.deliveries(Some("flow-read-attribution-v1")).await, 0);
    assert_eq!(f.deliveries(Some("other-consumer-v1")).await, 1);
    assert!(!f.funnel.accept_read_event(&event, true).await.unwrap());
    assert_eq!(
        f.funnel.report(7).await.unwrap().current.counts,
        Some(FlowFunnelCounts::default())
    );
    f.cleanup().await;
}

async fn blocked_by(pool: &PgPool, blocker: i32) {
    for _ in 0..100 {
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))",
        )
        .bind(blocker)
        .fetch_one(pool)
        .await
        .unwrap();
        if blocked {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("the test must witness a real PostgreSQL lock wait");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn concurrent_revoke_after_evidence_read_cannot_recreate_a_delivery() {
    let f = Fixture::new("revoke_race").await;
    let one = f.book().await;
    f.enter(one, 0, 240, 0).await;
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    let event = f.canonical(one, 0, 500, true).await;
    let mut lock = f.app.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    sqlx::query("SELECT subject_id FROM rec_subjects WHERE user_id=$1 FOR UPDATE")
        .bind(f.actor)
        .execute(&mut *lock)
        .await
        .unwrap();
    let store = f.funnel.clone();
    let pending = tokio::spawn(async move { store.accept_read_event(&event, true).await });
    blocked_by(&f.app, blocker).await;
    sqlx::query("UPDATE rec_subjects SET flow_epoch=NULL,flow_enabled_at=NULL WHERE user_id=$1")
        .bind(f.actor)
        .execute(&mut *lock)
        .await
        .unwrap();
    lock.commit().await.unwrap();
    assert!(!pending.await.unwrap().unwrap());
    assert_eq!(f.deliveries(None).await, 0);
    assert_eq!(f.scalar("SELECT count(*) FROM flow_book_cohorts").await, 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn receipt_delete_waits_for_processing_then_erases_its_delivery() {
    let f = Fixture::new("ttl_race").await;
    let one = f.book().await;
    f.enter(one, 0, 240, 0).await;
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    let event = f.canonical(one, 0, 500, true).await;
    let mut lock = f.app.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM flow_book_cohorts FOR UPDATE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let store = f.funnel.clone();
    let id = event.clone();
    let pending = tokio::spawn(async move { store.accept_read_event(&id, true).await });
    blocked_by(&f.app, blocker).await;
    let pool = f.app.clone();
    let id = event.clone();
    let mut deletion = tokio::spawn(async move {
        sqlx::query("DELETE FROM reader_flow_read_receipts WHERE event_id=$1::uuid")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap()
            .rows_affected()
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut deletion)
            .await
            .is_err(),
        "the C proof row must stay pinned until F finishes"
    );
    lock.rollback().await.unwrap();
    assert!(pending.await.unwrap().unwrap());
    assert_eq!(deletion.await.unwrap(), 1);
    assert_eq!(f.deliveries(None).await, 0);
    assert!(!f.funnel.accept_read_event(&event, true).await.unwrap());
    f.cleanup().await;
}
