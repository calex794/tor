use super::*;

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn late_committed_first_start_relocates_existing_anchor_without_event_replay() {
    let f = Fixture::new("late_canonical").await;
    let one = f.book().await;
    let earlier_entry = f.enter(one, 0, 240, 0).await;
    f.age(3600).await;
    f.funnel.cycle(true).await.unwrap();
    let later_entry = f.enter(one, 1, 240, 1).await;
    let later_chapter = f.chapter(one.id).await;
    let later = f.canonical(later_chapter, 0, 180, true).await;
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    let anchor: String = sqlx::query_scalar("SELECT cohort_id::text FROM flow_book_return_anchors")
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert_eq!(anchor, later_entry);
    // C's transaction occurrence precedes its commit. This canonical row
    // becomes visible after the later chapter already acquired an anchor.
    let earlier = f.canonical(one, 0, 4000, true).await;
    assert!(f.funnel.accept_read_event(&earlier, true).await.unwrap());
    let anchor: String = sqlx::query_scalar("SELECT cohort_id::text FROM flow_book_return_anchors")
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert_eq!(anchor, earlier_entry);
    assert_eq!(f.counts().await.new_readers, 1);
    // Restore the previously valid, now stale projection while retaining both
    // consumed deliveries. Maintenance must repair it without redelivery.
    let mut tx = f.app.begin().await.unwrap();
    sqlx::query(
        "UPDATE flow_book_return_anchors SET cohort_id=$1::uuid,
           started_at=(SELECT occurred_at FROM reader_flow_verified_read_events WHERE event_id=$2::uuid),
           expires_at=(SELECT expires_at FROM flow_book_cohorts WHERE id=$1::uuid)",
    ).bind(&later_entry).bind(&later).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "UPDATE flow_book_cohorts SET d7_state=CASE WHEN id=$1::uuid THEN 1 ELSE 0 END,
           new_reader=(id=$1::uuid)",
    )
    .bind(&later_entry)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    f.funnel.cycle(false).await.unwrap();
    let anchor: String = sqlx::query_scalar("SELECT cohort_id::text FROM flow_book_return_anchors")
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert_eq!(anchor, earlier_entry);
    assert_eq!(f.deliveries(Some("flow-read-attribution-v1")).await, 2);
    assert_eq!(f.counts().await.new_readers, 1);
    // An even earlier canonical start is unlinked and its command had FLOW
    // capture off. It removes the later anchor; no later cohort inherits it.
    let unlinked = f.chapter(one.id).await;
    f.canonical(unlinked, 0, 5000, false).await;
    f.funnel.cycle(false).await.unwrap();
    let counts = f.counts().await;
    assert_eq!(
        (counts.starts, counts.new_readers, counts.d7_anchors),
        (2, 0, 0)
    );
    assert_eq!(
        f.scalar("SELECT count(*) FROM flow_book_return_anchors")
            .await,
        0
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn earlier_start_in_same_cohort_rebases_the_target_day_anchor() {
    let f = Fixture::new("same_cohort_late").await;
    let one = f.book().await;
    f.enter(one, 0, 240, 0).await;
    let later = f.chapter(one.id).await;
    f.canonical(later, 0, 180, true).await;
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    let earlier = f.canonical(one, 0, 580, true).await;
    assert!(f.funnel.accept_read_event(&earlier, true).await.unwrap());
    let matching: bool = sqlx::query_scalar(
        "SELECT a.started_at=v.occurred_at AND c.first_start_event_id=v.event_id
         FROM flow_book_return_anchors a JOIN flow_book_cohorts c ON c.id=a.cohort_id
         JOIN reader_flow_verified_read_events v ON v.event_id=$1::uuid",
    )
    .bind(earlier)
    .fetch_one(&f.app)
    .await
    .unwrap();
    assert!(matching);
    let counts = f.counts().await;
    assert_eq!(
        (counts.starts, counts.d7_anchors, counts.new_readers),
        (1, 1, 1)
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn finalization_waits_until_all_65_changed_books_have_been_reconciled() {
    let f = Fixture::new("reconcile_batch").await;
    let mut books = Vec::new();
    for _ in 0..65 {
        let book = f.book().await;
        f.enter(book, 0, 240, 0).await;
        f.canonical(book, 0, 180, true).await;
        books.push(book);
    }
    f.age(360).await;
    for _ in 0..8 {
        f.funnel.cycle(true).await.unwrap();
        if f.counts().await.starts == 65 {
            break;
        }
    }
    assert_eq!(
        (f.counts().await.starts, f.counts().await.d7_anchors),
        (65, 65)
    );
    for book in books {
        let earlier_chapter = f.chapter(book.id).await;
        f.canonical(earlier_chapter, 0, 3600, false).await;
    }
    f.age(31 * 86_400).await;
    f.funnel.cycle(false).await.unwrap();
    let counts = f.counts().await;
    assert_eq!(
        (
            counts.cohorts,
            counts.starts,
            counts.new_readers,
            counts.d7_anchors
        ),
        (65, 65, 0, 0)
    );
    assert_eq!(f.scalar("SELECT count(*) FROM flow_book_cohorts").await, 0);
    assert!(!f.funnel.report(90).await.unwrap().worker_backlog);
    f.cleanup().await;
}
