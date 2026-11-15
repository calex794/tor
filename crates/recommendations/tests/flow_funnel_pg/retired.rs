use super::*;

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn retired_maintenance_freezes_late_c_facts_and_finalizes_expiry_once() {
    let f = Fixture::new("retired_maintenance").await;
    let book = f.book().await;
    f.enter(book, 0, 240, 0).await;
    let original = f.canonical(book, 0, 180, true).await;
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    assert_eq!(
        (f.counts().await.new_readers, f.counts().await.d7_pending),
        (1, 1)
    );
    f.age(8 * 86_400).await;
    let before: String = sqlx::query_scalar("SELECT row_to_json(c)::text FROM flow_book_cohorts c")
        .fetch_one(&f.app)
        .await
        .unwrap();
    let anchor: String =
        sqlx::query_scalar("SELECT row_to_json(a)::text FROM flow_book_return_anchors a")
            .fetch_one(&f.app)
            .await
            .unwrap();

    // A late canonical first read and newly available D7 activity would change
    // this projection under the former cycle(false) reconciliation path.
    let earlier_chapter = f.chapter(book.id).await;
    let earlier = f.canonical(earlier_chapter, 0, 9 * 86_400, false).await;
    let still_first: bool = sqlx::query_scalar(
        "SELECT first_for_book FROM reader_flow_verified_read_events WHERE event_id=$1::uuid",
    )
    .bind(&original)
    .fetch_one(&f.app)
    .await
    .unwrap();
    assert!(!still_first);
    sqlx::query(
        "INSERT INTO reader_flow_activity_days
           (subject_id,flow_epoch,book_id,activity_day,first_seen_at)
         SELECT subject_id,flow_epoch,book_id,(started_at AT TIME ZONE 'UTC')::date+7,
           (((started_at AT TIME ZONE 'UTC')::date+7)::timestamp+interval '12 hours') AT TIME ZONE 'UTC'
         FROM flow_book_return_anchors",
    )
    .execute(&f.app)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO event_deliveries(consumer,event_id) VALUES('retirement-control',$1::uuid)",
    )
    .bind(&original)
    .execute(&f.app)
    .await
    .unwrap();
    f.funnel.maintain_retired().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT row_to_json(c)::text FROM flow_book_cohorts c",)
            .fetch_one(&f.app)
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT row_to_json(a)::text FROM flow_book_return_anchors a",
        )
        .fetch_one(&f.app)
        .await
        .unwrap(),
        anchor
    );
    assert_eq!(f.deliveries(Some("flow-read-attribution-v1")).await, 1);
    let late_delivery: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries
         WHERE consumer='flow-read-attribution-v1' AND event_id=$1::uuid",
    )
    .bind(&earlier)
    .fetch_one(&f.app)
    .await
    .unwrap();
    assert_eq!(late_delivery, 0);

    f.age(23 * 86_400).await;
    let restarted = PgFlowFunnel::new(f.app.clone());
    let (one, two) = tokio::join!(f.funnel.maintain_retired(), restarted.maintain_retired());
    one.unwrap();
    two.unwrap();
    restarted.maintain_retired().await.unwrap();
    let archived = f.counts().await;
    assert_eq!(
        (
            archived.cohorts,
            archived.starts,
            archived.new_readers,
            archived.d7_anchors,
            archived.d7_pending,
            archived.d7_returned,
            archived.d7_unknown,
        ),
        (1, 1, 1, 1, 0, 0, 1),
        "the frozen contribution is archived once; an unresolved return is unknown"
    );
    assert_eq!(f.scalar("SELECT count(*) FROM flow_book_cohorts").await, 0);
    assert_eq!(
        f.scalar("SELECT count(*) FROM flow_book_return_anchors")
            .await,
        0
    );
    assert_eq!(f.deliveries(Some("flow-read-attribution-v1")).await, 0);
    assert_eq!(f.deliveries(Some("retirement-control")).await, 1);
    let canonical: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_chapter_starts WHERE user_id=$1")
            .bind(f.actor)
            .fetch_one(&f.app)
            .await
            .unwrap();
    assert_eq!(canonical, 2, "retirement never erases canonical C reading");
    assert!(!f.funnel.report(90).await.unwrap().worker_backlog);
    f.cleanup().await;
}
