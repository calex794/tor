use super::*;

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn discovery_uses_real_navigation_proof_latest_click_and_rec_tie_break() {
    let f = Fixture::new("discovery").await;
    let one = f.book().await;
    let another = f.book().await;
    let session = [31_u8; 32];
    let ticket = f
        .rec
        .issue_tickets(f.actor, &session, Surface::Home, feed(one.id))
        .await
        .unwrap()
        .items
        .remove(0)
        .ticket
        .unwrap();
    assert_eq!(
        f.rec.activate(f.actor, &session, &ticket).await.unwrap(),
        one.id
    );
    let recommendation = f
        .rec
        .record_book_open_with_flow(f.actor, &session, one.id)
        .await
        .unwrap()
        .unwrap();
    let organic_ticket = f
        .rec
        .issue_organic_tickets(f.actor, &session, OrganicSource::Search, &[one.id])
        .await
        .unwrap()
        .remove(0)
        .1;
    f.rec
        .activate_organic(f.actor, &session, &organic_ticket)
        .await
        .unwrap();
    let organic = f
        .rec
        .record_organic_book_open_with_flow(f.actor, &session, one.id)
        .await
        .unwrap()
        .unwrap();
    let mut actual = f.observation(one, 0, f.now().await, 0);
    let metadata = actual.book.as_mut().unwrap();
    metadata.rec_assignment = Some(recommendation.clone());
    metadata.organic_touch = Some(organic.clone());
    assert!(f.visits.observe(&actual).await.unwrap());
    let code: i16 = sqlx::query_scalar(
        "SELECT discovery_kind FROM flow_visit_receipts WHERE event_id=$1::uuid",
    )
    .bind(&actual.event_id)
    .fetch_one(&f.app)
    .await
    .unwrap();
    assert_eq!(code, 2, "the most recent qualifying click wins");
    assert!(!f.visits.observe(&actual).await.unwrap());
    let mut conflicting = actual.clone();
    conflicting.book.as_mut().unwrap().organic_touch = None;
    assert_eq!(
        f.visits.observe(&conflicting).await,
        Err(FlowVisitError::Conflict)
    );
    // Equal source time has one documented deterministic priority.
    let mut tx = f.app.begin().await.unwrap();
    for table in ["rec_assignments", "organic_discovery_touches"] {
        sqlx::query(&format!(
            "UPDATE {table} SET clicked_at=transaction_timestamp(),
            book_opened_at=transaction_timestamp() WHERE subject_id=$1::uuid"
        ))
        .bind(&f.scope.subject_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let mut tie = f.observation(one, 0, f.now().await, 3);
    tie.book.as_mut().unwrap().rec_assignment = Some(recommendation.clone());
    tie.book.as_mut().unwrap().organic_touch = Some(organic.clone());
    assert!(f.visits.observe(&tie).await.unwrap());
    let code: i16 = sqlx::query_scalar(
        "SELECT discovery_kind FROM flow_visit_receipts WHERE event_id=$1::uuid",
    )
    .bind(&tie.event_id)
    .fetch_one(&f.app)
    .await
    .unwrap();
    assert_eq!(code, 0);
    let mut wrong_book = f.observation(another, 0, f.now().await, 3);
    wrong_book.book.as_mut().unwrap().rec_assignment = Some(recommendation.clone());
    assert_eq!(
        f.visits.observe(&wrong_book).await,
        Err(FlowVisitError::Invalid)
    );
    let mut forged = f.observation(one, 0, f.now().await, 3);
    forged.book.as_mut().unwrap().rec_assignment = Some(new_ticket().unwrap());
    assert_eq!(
        f.visits.observe(&forged).await,
        Err(FlowVisitError::Invalid)
    );
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    let retained: i16 = sqlx::query_scalar("SELECT discovery_kind FROM flow_book_cohorts")
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert_eq!(
        retained, 2,
        "later navigation cannot rewrite the first book entry"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn pre_flow_click_cannot_be_reused_and_continuation_stays_unattributed() {
    let f = Fixture::new("boundaries").await;
    let one = f.book().await;
    let session = [43_u8; 32];
    let ticket = f
        .rec
        .issue_organic_tickets(f.actor, &session, OrganicSource::Catalog, &[one.id])
        .await
        .unwrap()
        .remove(0)
        .1;
    f.rec
        .activate_organic(f.actor, &session, &ticket)
        .await
        .unwrap();
    let proof = f
        .rec
        .record_organic_book_open_with_flow(f.actor, &session, one.id)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE rec_subjects SET flow_enabled_at=clock_timestamp() WHERE user_id=$1")
        .bind(f.actor)
        .execute(&f.app)
        .await
        .unwrap();
    assert!(f
        .rec
        .record_organic_book_open_with_flow(f.actor, &session, one.id)
        .await
        .unwrap()
        .is_none());
    let mut old = f.observation(one, 0, f.now().await, 3);
    old.book.as_mut().unwrap().organic_touch = Some(proof);
    assert_eq!(f.visits.observe(&old).await, Err(FlowVisitError::Invalid));
    // An origin that already crossed its FLOW02 TTL has no attributable source
    // or visit ID. Book navigation can be counted only as an unmatched receipt.
    sqlx::query(
        "INSERT INTO flow_visit_continuations(subject_id,flow_epoch,last_at)
         VALUES($1::uuid,$2::uuid,now()-interval '1 minute')",
    )
    .bind(&f.scope.subject_id)
    .bind(&f.scope.epoch)
    .execute(&f.app)
    .await
    .unwrap();
    f.enter(one, 0, 0, 3).await;
    f.age(360).await;
    f.funnel.cycle(true).await.unwrap();
    assert_eq!(f.scalar("SELECT count(*) FROM flow_book_cohorts").await, 0);
    let report = f.funnel.report(7).await.unwrap();
    assert_eq!(report.current.pending_book_observations, Some(0));
    assert_eq!(report.current.unmatched_book_observations, Some(1));
    assert!(report.current.counts.is_none());
    f.cleanup().await;
}
