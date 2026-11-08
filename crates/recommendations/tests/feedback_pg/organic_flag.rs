use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn organic_flag_off_does_not_collect_even_with_rec_consent_on() {
    let (pool, store, actor, book) = fixture("organic-off-rec-on").await;
    enable(&store, actor).await;
    // An earlier organic click can exist when the worker flag is switched
    // off. A later REC click still deserves its ordinary attribution.
    let organic = store
        .issue_organic_tickets(actor, &[7; 32], OrganicSource::Catalog, &[book])
        .await
        .unwrap();
    store
        .activate_organic(actor, &[7; 32], &organic[0].1)
        .await
        .unwrap();
    store
        .record_organic_book_open(actor, &[7; 32], book)
        .await
        .unwrap();
    let digest = tor_recommendations::domain::feedback::token_digest(&organic[0].1).unwrap();
    sqlx::query(
        "UPDATE organic_discovery_touches
         SET created_at=now()-interval '20 seconds',
             clicked_at=now()-interval '10 seconds'
         WHERE ticket_digest=$1",
    )
    .bind(digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let rec = assignment(&store, actor, book).await;
    store.activate(actor, &[7; 32], &rec).await.unwrap();

    let event = canonical_start(&pool, actor, book).await;
    // G drains only 64 events per batch; older tests may leave other receipts.
    for _ in 0..8 {
        store.tick_feedback(true, false).await.unwrap();
        let processed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM event_deliveries
             WHERE consumer='recommendation-feedback-v1' AND event_id=$1::uuid)",
        )
        .bind(&event)
        .fetch_one(&pool)
        .await
        .unwrap();
        if processed {
            break;
        }
    }
    let rec_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rec_first_conversions WHERE source_event_id=$1::uuid",
    )
    .bind(&event)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rec_count, 1, "ordinary REC attribution still works");
    for table in [
        "organic_discovery_first_conversions",
        "organic_discovery_unattributed_reads",
    ] {
        let query = format!("SELECT count(*) FROM {table} WHERE source_event_id=$1::uuid");
        let n: i64 = sqlx::query_scalar(&query)
            .bind(&event)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "organic OFF must not collect in {table}");
    }

    // If the last navigation was organic before a worker restart, turning
    // organic OFF must not silently credit an earlier recommendation click.
    let (older_pool, store, actor, book) = fixture("organic-off-last-touch").await;
    enable(&store, actor).await;
    let rec = assignment(&store, actor, book).await;
    store.activate(actor, &[7; 32], &rec).await.unwrap();
    sqlx::query(
        "UPDATE rec_assignments
         SET created_at=now()-interval '20 seconds',
             clicked_at=now()-interval '10 seconds'
         WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id=$1)
           AND book_id=$2",
    )
    .bind(actor)
    .bind(book)
    .execute(&older_pool)
    .await
    .unwrap();
    let organic = store
        .issue_organic_tickets(actor, &[7; 32], OrganicSource::Search, &[book])
        .await
        .unwrap();
    store
        .activate_organic(actor, &[7; 32], &organic[0].1)
        .await
        .unwrap();
    store
        .record_organic_book_open(actor, &[7; 32], book)
        .await
        .unwrap();
    let event = canonical_start(&older_pool, actor, book).await;
    for _ in 0..8 {
        store.tick_feedback(true, false).await.unwrap();
        let delivered: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM event_deliveries
             WHERE consumer='recommendation-feedback-v1' AND event_id=$1::uuid)",
        )
        .bind(&event)
        .fetch_one(&older_pool)
        .await
        .unwrap();
        if delivered {
            break;
        }
    }
    let delivered: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM event_deliveries
         WHERE consumer='recommendation-feedback-v1' AND event_id=$1::uuid)",
    )
    .bind(&event)
    .fetch_one(&older_pool)
    .await
    .unwrap();
    assert!(delivered, "OFF still acknowledges authentic C events");
    for table in [
        "rec_first_conversions",
        "organic_discovery_first_conversions",
        "organic_discovery_unattributed_reads",
    ] {
        let query = format!("SELECT count(*) FROM {table} WHERE source_event_id=$1::uuid");
        let count: i64 = sqlx::query_scalar(&query)
            .bind(&event)
            .fetch_one(&older_pool)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "newer suppressed organic click must not credit {table}"
        );
    }

    let (direct_pool, store, actor, book) = fixture("organic-off-direct").await;
    enable(&store, actor).await;
    let event = canonical_start(&direct_pool, actor, book).await;
    for _ in 0..8 {
        store.tick_feedback(true, false).await.unwrap();
        let processed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM event_deliveries
             WHERE consumer='recommendation-feedback-v1' AND event_id=$1::uuid)",
        )
        .bind(&event)
        .fetch_one(&direct_pool)
        .await
        .unwrap();
        if processed {
            break;
        }
    }
    let delivered: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM event_deliveries
         WHERE consumer='recommendation-feedback-v1' AND event_id=$1::uuid)",
    )
    .bind(&event)
    .fetch_one(&direct_pool)
    .await
    .unwrap();
    assert!(delivered, "organic-OFF worker must still receipt C events");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM organic_discovery_unattributed_reads
         WHERE source_event_id=$1::uuid",
    )
    .bind(&event)
    .fetch_one(&direct_pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "direct C read must not create organic-off data");
}
