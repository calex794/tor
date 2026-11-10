//! Separate FLOW permission, CAS/replay, revocation and account erasure.
use super::*;
use tor_recommendations::services::visits::FlowScope;

async fn scope(store: &PgRecommendations, actor: i64) -> FlowScope {
    store.flow_scope(actor).await.unwrap().unwrap()
}

async fn scope_rows(pool: &PgPool, subject: &str) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM flow_visits WHERE subject_id=$1::uuid),
                (SELECT count(*) FROM flow_visit_receipts WHERE subject_id=$1::uuid),
                (SELECT count(*) FROM flow_visit_account_days WHERE subject_id=$1::uuid),
                (SELECT count(*) FROM flow_visit_continuations WHERE subject_id=$1::uuid)",
    )
    .bind(subject)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn daily(pool: &PgPool, date: &str) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT visits,pageviews,observed_seconds FROM flow_visit_daily
         WHERE cohort_day=$1::date AND acquisition_hint=3
           AND landing_surface=4 AND last_surface=4",
    )
    .bind(date)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Two visits, one already finalized, and one account-day observation.
/// These are explicit storage fixtures, not evidence of browser collection.
async fn seed_scope(pool: &PgPool, scope: &FlowScope) -> String {
    let event: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    let date: String = sqlx::query_scalar(
        "INSERT INTO flow_visits(subject_id,flow_epoch,started_at,last_at,
             first_event_id,last_event_id,landing_surface,last_surface,
             acquisition_hint,pageviews,finalized_at)
         VALUES($1::uuid,$2::uuid,now()-interval '2 hours',now()-interval '115 minutes',
             $3::uuid,gen_random_uuid(),4,4,3,2,now()-interval '90 minutes')
         RETURNING (started_at AT TIME ZONE 'UTC')::date::text",
    )
    .bind(&scope.subject_id)
    .bind(&scope.epoch)
    .bind(&event)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO flow_visit_daily(cohort_day,acquisition_hint,landing_surface,
             last_surface,visits,pageviews,observed_seconds)
         VALUES($1::date,3,4,4,1,2,300)
         ON CONFLICT(cohort_day,acquisition_hint,landing_surface,last_surface)
         DO UPDATE SET visits=flow_visit_daily.visits+1,
             pageviews=flow_visit_daily.pageviews+2,
             observed_seconds=flow_visit_daily.observed_seconds+300",
    )
    .bind(&date)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO flow_visits(subject_id,flow_epoch,started_at,last_at,
             first_event_id,last_event_id,landing_surface,last_surface,
             acquisition_hint,pageviews)
         VALUES($1::uuid,$2::uuid,now(),now(),$3::uuid,$3::uuid,0,0,0,1)",
    )
    .bind(&scope.subject_id)
    .bind(&scope.epoch)
    .bind(&event)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO flow_visit_receipts(event_id,subject_id,flow_epoch,
             observed_at,surface,acquisition_hint)
         VALUES($3::uuid,$1::uuid,$2::uuid,now(),0,0)",
    )
    .bind(&scope.subject_id)
    .bind(&scope.epoch)
    .bind(&event)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO flow_visit_account_days(subject_id,flow_epoch,bucket_date)
         VALUES($1::uuid,$2::uuid,(now() AT TIME ZONE 'UTC')::date)",
    )
    .bind(&scope.subject_id)
    .bind(&scope.epoch)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO flow_visit_continuations(subject_id,flow_epoch,last_at)
         VALUES($1::uuid,$2::uuid,now())",
    )
    .bind(&scope.subject_id)
    .bind(&scope.epoch)
    .execute(pool)
    .await
    .unwrap();
    date
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn rec13_flow_opt_in_is_explicit_cas_and_replay_safe() {
    let (pool, store, actor, _) = fixture("flow-consent").await;
    assert!(!store.flow_consent(actor).await.unwrap().enabled);
    assert!(store.flow_scope(actor).await.unwrap().is_none());
    assert_eq!(
        store
            .set_flow_consent(actor, true, 0, &new_ticket().unwrap())
            .await,
        Err(FeedbackError::Forbidden)
    );
    assert_eq!(store.flow_consent(actor).await.unwrap().revision, 0);
    enable(&store, actor).await;
    assert!(!store.flow_consent(actor).await.unwrap().enabled);
    assert!(store.flow_scope(actor).await.unwrap().is_none());
    let command = new_ticket().unwrap();
    let enabled = store
        .set_flow_consent(actor, true, 1, &command)
        .await
        .unwrap();
    assert!(enabled.enabled);
    assert_eq!(enabled.revision, 2);
    let original = scope(&store, actor).await;
    assert_eq!(
        store.set_flow_consent(actor, true, 1, &command).await,
        Ok(enabled)
    );
    assert_eq!(
        store.set_flow_consent(actor, false, 1, &command).await,
        Err(FeedbackError::IdempotencyConflict)
    );
    assert_eq!(
        store.set_consent(actor, true, 1, &command).await,
        Err(FeedbackError::IdempotencyConflict)
    );
    assert_eq!(
        store
            .set_flow_consent(actor, true, 1, &new_ticket().unwrap())
            .await,
        Err(FeedbackError::Conflict)
    );
    assert_eq!(
        store
            .set_flow_consent(actor, true, 2, &new_ticket().unwrap())
            .await
            .unwrap()
            .revision,
        3
    );
    assert_eq!(scope(&store, actor).await.epoch, original.epoch);
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.flow_scope(actor).await.unwrap().is_none());
    assert_eq!(
        store
            .set_flow_consent(actor, true, 3, &new_ticket().unwrap())
            .await,
        Err(FeedbackError::Forbidden)
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn rec13_flow_revoke_erases_raw_and_retracts_retained_aggregate() {
    let (pool, store, actor, _) = fixture("flow-revoke").await;
    enable(&store, actor).await;
    store
        .set_flow_consent(actor, true, 1, &new_ticket().unwrap())
        .await
        .unwrap();
    let original = scope(&store, actor).await;
    let date = seed_scope(&pool, &original).await;
    assert_eq!(scope_rows(&pool, &original.subject_id).await, (2, 1, 1, 1));
    let before = daily(&pool, &date).await;
    let command = new_ticket().unwrap();
    store
        .set_flow_consent(actor, false, 2, &command)
        .await
        .unwrap();
    assert_eq!(scope_rows(&pool, &original.subject_id).await, (0, 0, 0, 0));
    assert_eq!(
        daily(&pool, &date).await,
        (before.0 - 1, before.1 - 2, before.2 - 300)
    );
    assert!(store.consent(actor).await.unwrap().enabled);
    assert!(store.flow_scope(actor).await.unwrap().is_none());
    // A lost-response retry does not subtract the original contribution twice.
    assert_eq!(
        store
            .set_flow_consent(actor, false, 2, &command)
            .await
            .unwrap()
            .revision,
        3
    );
    store
        .set_flow_consent(actor, true, 3, &new_ticket().unwrap())
        .await
        .unwrap();
    let next = scope(&store, actor).await;
    assert_eq!(next.subject_id, original.subject_id);
    assert_ne!(next.epoch, original.epoch);
    // A stale queued scope cannot reinsert personal rows after a new opt-in.
    let stale = sqlx::query(
        "INSERT INTO flow_visit_account_days(subject_id,flow_epoch,bucket_date)
         VALUES($1::uuid,$2::uuid,(now() AT TIME ZONE 'UTC')::date)",
    )
    .bind(&original.subject_id)
    .bind(&original.epoch)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        stale.as_database_error().unwrap().code().as_deref(),
        Some("23503")
    );
    let date = seed_scope(&pool, &next).await;
    let before = daily(&pool, &date).await;
    // General REC opt-out also erases FLOW before its UUID subject rotation.
    store
        .set_consent(actor, false, 4, &new_ticket().unwrap())
        .await
        .unwrap();
    assert_eq!(scope_rows(&pool, &next.subject_id).await, (0, 0, 0, 0));
    assert_eq!(
        daily(&pool, &date).await,
        (before.0 - 1, before.1 - 2, before.2 - 300)
    );
    assert!(!store.flow_consent(actor).await.unwrap().enabled);
    store
        .set_consent(actor, true, 5, &new_ticket().unwrap())
        .await
        .unwrap();
    assert!(store.flow_scope(actor).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn rec13_flow_parallel_consent_and_account_deletion_are_serialized() {
    let (pool, store, actor, _) = fixture("flow-delete").await;
    enable(&store, actor).await;
    let first = new_ticket().unwrap();
    let second = new_ticket().unwrap();
    let (left, right) = tokio::join!(
        store.set_flow_consent(actor, true, 1, &first),
        store.set_flow_consent(actor, true, 1, &second)
    );
    assert!(matches!(
        (&left, &right),
        (Ok(_), Err(FeedbackError::Conflict)) | (Err(FeedbackError::Conflict), Ok(_))
    ));
    assert_eq!(store.flow_consent(actor).await.unwrap().revision, 2);
    let active = scope(&store, actor).await;
    let date = seed_scope(&pool, &active).await;
    let before = daily(&pool, &date).await;
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(scope_rows(&pool, &active.subject_id).await, (0, 0, 0, 0));
    assert_eq!(
        daily(&pool, &date).await,
        (before.0 - 1, before.1 - 2, before.2 - 300)
    );
}
