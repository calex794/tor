use super::*;

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn replay_two_workers_restart_off_worker_and_parallel_outcomes() {
    let f = Fixture::new("replay").await;
    let one = f.book().await;
    let two = f.book().await;
    f.enter(one, 0, 240, 0).await;
    f.enter(one, 1, 220, 3).await;
    f.canonical(one, 0, 200, true).await;
    f.canonical(one, 1, 180, true).await;
    let next_chapter = f.chapter(one.id).await;
    f.canonical(next_chapter, 0, 160, true).await;
    f.enter(two, 0, 140, 3).await;
    for kind in [1_i16, 2] {
        sqlx::query(
            "INSERT INTO reader_flow_book_actions(subject_id,flow_epoch,book_id,kind,occurred_at)
             VALUES($1::uuid,$2::uuid,$3,$4,now()-interval '2 minutes')",
        )
        .bind(&f.scope.subject_id)
        .bind(&f.scope.epoch)
        .bind(one.id)
        .bind(kind)
        .execute(&f.app)
        .await
        .unwrap();
    }
    f.age(360).await;
    f.funnel.cycle(false).await.unwrap();
    assert_eq!(f.scalar("SELECT count(*) FROM flow_book_cohorts").await, 0);
    assert_eq!(
        f.scalar("SELECT count(*) FROM flow_visit_receipts WHERE funnel_processed")
            .await,
        0
    );
    assert_eq!(f.deliveries(None).await, 0);
    let restarted = PgFlowFunnel::new(f.app.clone());
    let (a, b) = tokio::join!(f.funnel.cycle(true), restarted.cycle(true));
    a.unwrap();
    b.unwrap();
    let before = f.counts().await;
    assert_eq!(
        before,
        FlowFunnelCounts {
            cohorts: 2,
            detail_opens: 2,
            reader_opens: 1,
            starts: 1,
            completions: 1,
            saves: 1,
            follows: 1,
            new_readers: 1,
            d7_anchors: 1,
            d7_pending: 1,
            d7_returned: 0,
            d7_unknown: 0
        }
    );
    assert_eq!(f.deliveries(Some("flow-read-attribution-v1")).await, 3);
    let action_receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM flow_book_action_receipts WHERE subject_id=$1::uuid",
    )
    .bind(&f.scope.subject_id)
    .fetch_one(&f.app)
    .await
    .unwrap();
    assert_eq!(action_receipts, 2);
    restarted.cycle(true).await.unwrap();
    assert_eq!(f.counts().await, before);
    assert!(!f.funnel.report(7).await.unwrap().worker_backlog);
    assert_eq!(f.funnel.report(2).await, Err(FlowFunnelError::Invalid));
    assert!(f.funnel.report(7).await.unwrap().previous.counts.is_none());
    sqlx::query("UPDATE books SET visibility='draft',admin_hold=true WHERE id=ANY($1)")
        .bind(vec![one.id, two.id])
        .execute(&f.app)
        .await
        .unwrap();
    assert_eq!(
        f.counts().await,
        before,
        "later visibility cannot rewrite numeric authorized history"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn completion_first_is_bound_to_its_chapter_start_not_a_later_visit() {
    let f = Fixture::new("completion_first").await;
    let one = f.book().await;
    let first_entry = f.enter(one, 0, 240, 0).await;
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    let first_start = f.canonical(one, 0, 500, true).await;
    f.age(3600).await;
    let second_entry = f.enter(one, 1, 180, 1).await;
    let second_chapter = f.chapter(one.id).await;
    let second_start = f.canonical(second_chapter, 0, 140, true).await;
    let completion = f.canonical(one, 1, 120, true).await;
    f.age(360).await;
    assert!(f.funnel.accept_read_event(&completion, true).await.unwrap());
    assert!(!f
        .funnel
        .accept_read_event(&first_start, true)
        .await
        .unwrap());
    f.funnel.cycle(true).await.unwrap();
    let states: Vec<(String, bool, bool, String)> = sqlx::query_as(
        "SELECT id::text,started,completed,first_start_event_id::text FROM flow_book_cohorts ORDER BY first_entry_at",
    ).fetch_all(&f.app).await.unwrap();
    assert_eq!(
        states,
        vec![
            (first_entry, true, true, first_start),
            (second_entry, true, false, second_start)
        ]
    );
    let counts = f.counts().await;
    assert_eq!(
        (
            counts.cohorts,
            counts.starts,
            counts.completions,
            counts.new_readers
        ),
        (2, 2, 1, 1)
    );
    assert_eq!(
        counts.d7_anchors, 1,
        "later chapter start never steals the first anchor"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn canonical_off_command_and_naked_outbox_cannot_manufacture_flow_reading() {
    let f = Fixture::new("proofs").await;
    let one = f.book().await;
    f.enter(one, 0, 240, 3).await;
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    let off_start = f.canonical(one, 0, 500, false).await;
    let captured_completion = f.canonical(one, 1, 400, true).await;
    assert!(!f.funnel.accept_read_event(&off_start, true).await.unwrap());
    assert!(!f
        .funnel
        .accept_read_event(&captured_completion, false)
        .await
        .unwrap());
    assert_eq!(f.deliveries(None).await, 0);
    assert!(f
        .funnel
        .accept_read_event(&captured_completion, true)
        .await
        .unwrap());
    assert_eq!(
        (f.counts().await.starts, f.counts().await.completions),
        (0, 0)
    );
    let forged: String = sqlx::query_scalar(
        "INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
         SELECT event_type,schema_version,aggregate_kind,aggregate_id,gen_random_uuid()::text,payload,required_consumers
         FROM outbox_events WHERE id=$1::uuid RETURNING id::text",
    ).bind(&off_start).fetch_one(&f.app).await.unwrap();
    assert_eq!(
        f.funnel.accept_read_event(&forged, true).await,
        Err(FlowFunnelError::Invalid)
    );
    assert_eq!(f.deliveries(None).await, 1);
    assert_eq!(
        f.scalar("SELECT count(*) FROM flow_book_return_anchors")
            .await,
        0
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn queued_late_bridge_seals_one_stable_book_cohort() {
    let f = Fixture::new("late_bridge").await;
    let one = f.book().await;
    let earliest = f.enter(one, 0, 240, 0).await;
    f.age(28 * 60).await;
    f.enter(one, 1, 120, 3).await;
    assert_eq!(f.scalar("SELECT count(*) FROM flow_visits").await, 2);
    f.enter(one, 0, 180, 1).await;
    assert_eq!(f.scalar("SELECT count(*) FROM flow_visits").await, 1);
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    let actual: (String, i16, bool, bool) = sqlx::query_as(
        "SELECT id::text,acquisition_hint,has_detail,has_reader FROM flow_book_cohorts",
    )
    .fetch_one(&f.app)
    .await
    .unwrap();
    assert_eq!(actual, (earliest, 0, true, true));
    assert_eq!(
        f.scalar("SELECT count(*) FROM flow_visit_receipts WHERE cohort_id IS NOT NULL")
            .await,
        3
    );
    f.funnel.cycle(true).await.unwrap();
    assert_eq!(f.counts().await.cohorts, 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn inactive_queued_proofs_cannot_starve_the_next_active_event() {
    let inactive = Fixture::new("inactive_queue").await;
    let book = inactive.book().await;
    let first = inactive.canonical(book, 0, 1200, true).await;
    for _ in 1..64 {
        let chapter = inactive.chapter(book.id).await;
        inactive.canonical(chapter, 0, 1200, true).await;
    }
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(inactive.actor)
        .execute(&inactive.app)
        .await
        .unwrap();
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_flow_verified_read_receipts WHERE subject_id=$1::uuid",
    )
    .bind(&inactive.scope.subject_id)
    .fetch_one(&inactive.app)
    .await
    .unwrap();
    assert_eq!(
        queued, 64,
        "these real captured proofs fill the whole read batch"
    );

    let active = Fixture::new("active_after_queue").await;
    let current_book = active.book().await;
    active.enter(current_book, 0, 240, 3).await;
    active.canonical(current_book, 0, 180, true).await;
    active.age(360).await;
    active.funnel.cycle(true).await.unwrap();
    assert_eq!(
        active.counts().await.starts,
        1,
        "eligible work behind the inactive batch advances"
    );
    assert_eq!(active.deliveries(Some("flow-read-attribution-v1")).await, 1);
    let inactive_deliveries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_deliveries d JOIN outbox_events e ON e.id=d.event_id
         WHERE d.consumer='flow-read-attribution-v1' AND e.payload->>'user_id'=$1",
    )
    .bind(inactive.actor.to_string())
    .fetch_one(&active.app)
    .await
    .unwrap();
    assert_eq!(inactive_deliveries, 0);
    assert!(!active.funnel.accept_read_event(&first, true).await.unwrap());
    assert!(!active.funnel.report(7).await.unwrap().worker_backlog);
    active.cleanup().await;
    inactive.cleanup().await;
}
