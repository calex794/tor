//! Full C -> F acceptance in disposable PostgreSQL. No synthetic HTTP reading.
//! Tests call the canonical validated C CAS, including its durable outbox.
#[path = "surprise_experiment_pg/fixture.rs"]
mod fixture;
use fixture::Fixture;
use tor_recommendations::repositories::surprises::{PlanState, SurpriseError};

#[tokio::test]
#[ignore = "requires migrated disposable tor_ci PostgreSQL"]
async fn experiment_uses_canonical_reading_keeps_itt_and_fails_closed() {
    let mut f = Fixture::new().await;
    let control = f.user().await;
    let gentle = f.user().await;
    let standard = f.user().await;
    let unopened = f.user().await;
    let old_book = f.book().await;
    let new_book = f.book().await;
    let activity = f.book().await;
    let key = [0x43; 32];
    assert_eq!(
        f.surprises.plan(standard, &key).await.unwrap(),
        PlanState::Off
    );
    f.advance(standard, old_book, 1, 1, 0).await;
    f.start().await;
    let first_run = f.run;
    for (user, arm) in [
        (control, "control"),
        (gentle, "gentle"),
        (standard, "standard"),
        (unopened, "standard"),
    ] {
        f.enroll(user, arm).await;
    }
    assert_eq!(
        f.surprises.plan(control, &key).await.unwrap(),
        PlanState::NotSelected
    );
    let PlanState::Planned { utc_day, .. } = f.surprises.plan(standard, &key).await.unwrap() else {
        panic!("standard at 1000 per mille must be planned");
    };
    let before = f.surprises.plan(standard, &key).await.unwrap();
    assert_eq!(
        f.surprises.plan(standard, &[0x99; 32]).await.unwrap(),
        before
    );
    assert!(!f.surprises.claim(control, utc_day, 1200).await.unwrap());
    assert!(f.surprises.open(control, utc_day).await.unwrap().is_none());
    let assignment: String = sqlx::query_scalar(
        "SELECT arm FROM surprise_experiment_participants WHERE run_id=$1 AND user_id=$2",
    )
    .bind(f.run)
    .bind(standard)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        assignment, "standard",
        "key rotation cannot reshuffle an enrolled account"
    );

    // Approved/opened content is an operational event, never reading proof.
    let content: i64 = sqlx::query_scalar(
        "INSERT INTO surprise_content(kind,title,body,state)
        VALUES('joke',$1,'No reading credit for this gift','approved') RETURNING id",
    )
    .bind(&f.prefix)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE surprise_daily_decisions SET created_at=now()-interval '30 minutes' WHERE user_id=$1")
        .bind(standard).execute(&f.pool).await.unwrap();
    assert!(!f.surprises.claim(control, utc_day, 1200).await.unwrap());
    assert!(f.surprises.claim(standard, utc_day, 1200).await.unwrap());
    assert!(f.surprises.open(standard, utc_day).await.unwrap().is_some());
    assert_eq!(f.bits(standard).await, (false, false, false, false, false));

    // Completion of a previously started book is a guardrail, not a new-book success.
    f.advance(standard, old_book, 2, 8, 1).await;
    assert_eq!(f.bits(standard).await, (false, true, false, false, false));
    f.advance(standard, new_book, 1, 0, 0).await;
    f.advance(standard, new_book, 1, 1, 1).await;
    assert!(
        !f.bits(standard).await.0,
        "a start/positive cursor is not meaningful completion"
    );
    f.advance(standard, new_book, 2, 8, 2).await;
    assert!(f.bits(standard).await.0);
    f.advance(control, new_book, 2, 8, 0).await;
    assert!(
        f.bits(control).await.0,
        "control reading is captured without any gift"
    );
    let report = f
        .surprises
        .admin_reading_experiment(&f.digest)
        .await
        .unwrap()
        .unwrap();
    assert!(
        report.differences.is_none(),
        "never declare uplift while enrollment is open"
    );
    assert!(matches!(
        f.surprises.admin_reading_experiment(&[0x18; 32]).await,
        Err(SurpriseError::Forbidden)
    ));

    // A forged completion with an invalid authority cannot mint a primary success.
    sqlx::query("WITH c AS (
        INSERT INTO reader_chapter_completions(user_id,book_id,chapter_id,first_version_id,source_event_id)
        VALUES($1,$2,$3,$4,gen_random_uuid()) RETURNING source_event_id
      ) INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
      SELECT source_event_id,'ChapterCompletedV1',1,'chapter',$3,concat('fake-surprise:',$1),
        jsonb_build_object('user_id',$1,'book_id',$2,'chapter_id',$3,'version_id',$4,'authority','client-click'),
        ARRAY['ranking-eligibility-v1'] FROM c")
        .bind(gentle).bind(new_book.id).bind(new_book.chapter).bind(new_book.version)
        .execute(&f.pool).await.unwrap();
    assert!(!f.bits(gentle).await.0);

    sqlx::query("UPDATE surprise_experiment_participants SET enrolled_at=clock_timestamp()-interval '25 hours' WHERE run_id=$1 AND user_id=$2")
        .bind(f.run).bind(standard).execute(&f.pool).await.unwrap();
    f.advance(standard, activity, 1, 1, 0).await;
    assert!(f.bits(standard).await.2);
    sqlx::query("UPDATE surprise_experiment_participants SET enrolled_at=clock_timestamp()-interval '169 hours' WHERE run_id=$1 AND user_id=$2")
        .bind(f.run).bind(standard).execute(&f.pool).await.unwrap();
    f.advance(standard, activity, 1, 2, 1).await;
    assert!(f.bits(standard).await.3);
    f.surprises.disable(standard).await.unwrap();
    assert!(f.bits(standard).await.4);
    sqlx::query("UPDATE surprise_experiment_participants SET enrolled_at=clock_timestamp()-interval '216 hours' WHERE run_id=$1")
        .bind(f.run).execute(&f.pool).await.unwrap();
    f.surprises.maintain_experiment().await.unwrap();
    let report = f
        .surprises
        .admin_reading_experiment(&f.digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report.counts.iter().map(|arm| arm.enrolled).sum::<i64>(), 4);
    assert_eq!(
        report.counts[2].enrolled, 2,
        "unopened and disabled remain in ITT"
    );
    assert_eq!(report.counts[2].meaningful, 1);
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(unopened)
        .execute(&f.pool)
        .await
        .unwrap();
    f.surprises.maintain_experiment().await.unwrap();
    let report = f
        .surprises
        .admin_reading_experiment(&f.digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        report.counts[2].enrolled, 2,
        "account deletion cannot shrink a denominator"
    );

    // A real optional-capture lock timeout must preserve the C command and
    // permanently invalidate the trial; missing capture is not a negative outcome.
    sqlx::query("UPDATE surprise_experiment_participants SET enrolled_at=clock_timestamp()-interval '25 hours' WHERE run_id=$1 AND user_id=$2")
        .bind(f.run).bind(standard).execute(&f.pool).await.unwrap();
    sqlx::query("UPDATE surprise_user_preferences SET disabled=false WHERE user_id=$1")
        .bind(standard)
        .execute(&f.pool)
        .await
        .unwrap();
    let mut blocker = f.pool.begin().await.unwrap();
    sqlx::query(
        "SELECT id FROM surprise_experiment_participants WHERE run_id=$1 AND user_id=$2 FOR UPDATE",
    )
    .bind(f.run)
    .bind(standard)
    .execute(&mut *blocker)
    .await
    .unwrap();
    f.advance(standard, activity, 1, 3, 2).await;
    blocker.rollback().await.unwrap();
    let healthy: bool = sqlx::query_scalar(
        "SELECT faults_start=surprise_capture_faults() FROM surprise_experiment_runs WHERE id=$1",
    )
    .bind(f.run)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(!healthy);
    let allowed: bool = sqlx::query_scalar("SELECT surprise_delivery_allowed($1,$2)")
        .bind(standard)
        .bind(utc_day)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(
        !allowed,
        "capture fault fences delivery before the next worker tick"
    );
    f.surprises.maintain_experiment().await.unwrap();
    let stopped = f
        .surprises
        .admin_reading_experiment(&f.digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.decision, "capture_failure");
    let flags: (bool, bool) = sqlx::query_as(
        "SELECT enabled,experiment_enabled FROM surprise_runtime_policy WHERE singleton",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(flags, (false, false));
    assert!(f.surprises.open(standard, utc_day).await.unwrap().is_none());

    f.start().await;
    assert_ne!(f.run, first_run);
    assert_eq!(
        f.surprises.plan(control, &key).await.unwrap(),
        PlanState::NotSelected,
        "a new run must not recycle an older daily decision"
    );
    sqlx::query("UPDATE surprise_experiment_runs SET last_checked_at=now()-interval '3 minutes' WHERE id=$1")
        .bind(f.run).execute(&f.pool).await.unwrap();
    let fresh = f.user().await;
    assert_eq!(
        f.surprises.plan(fresh, &key).await.unwrap(),
        PlanState::NotSelected
    );
    f.surprises.maintain_experiment().await.unwrap();
    let stopped = f
        .surprises
        .admin_reading_experiment(&f.digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.decision, "monitor_gap");

    // Collection OFF does not prevent bounded deletion; frozen totals survive.
    let total_before: i64 = sqlx::query_scalar(
        "SELECT sum(enrolled)::bigint FROM surprise_experiment_totals WHERE run_id=$1",
    )
    .bind(first_run)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE surprise_experiment_participants SET enrolled_at=now()-interval '31 days' WHERE run_id=$1")
        .bind(first_run).execute(&f.pool).await.unwrap();
    sqlx::query(
        "UPDATE surprise_daily_decisions SET utc_day=utc_day-31 WHERE experiment_run_id=$1",
    )
    .bind(first_run)
    .execute(&f.pool)
    .await
    .unwrap();
    assert!(f.surprises.maintain_experiment().await.unwrap());
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM surprise_experiment_participants WHERE run_id=$1")
            .bind(first_run)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
    let total_after: i64 = sqlx::query_scalar(
        "SELECT sum(enrolled)::bigint FROM surprise_experiment_totals WHERE run_id=$1",
    )
    .bind(first_run)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(total_before, total_after);
    let cap: bool = sqlx::query_scalar(
        "SELECT last_shown_at IS NOT NULL FROM surprise_user_preferences WHERE user_id=$1",
    )
    .bind(standard)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(cap, "retention must not clear the user's frequency cap");

    f.start().await;
    sqlx::query(
        "UPDATE surprise_experiment_runs SET started_at=now()-interval '23 days',
        enrollment_ends_at=now()-interval '9 days',ends_at=now()-interval '1 day' WHERE id=$1",
    )
    .bind(f.run)
    .execute(&f.pool)
    .await
    .unwrap();
    f.surprises.maintain_experiment().await.unwrap();
    let final_report = f
        .surprises
        .admin_reading_experiment(&f.digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(final_report.state, "completed");
    assert_eq!(final_report.decision, "insufficient_sample");
    assert!(final_report.differences.is_none());

    f.start().await;
    sqlx::query(
        "INSERT INTO surprise_experiment_participants(run_id,arm,enrolled_at,returned_d7)
        SELECT $1,CASE WHEN i<=1000 THEN 'control' WHEN i<=3000 THEN 'gentle' ELSE 'standard' END,
          now()-interval '9 days',i<=1000 FROM generate_series(1,5000) i",
    )
    .bind(f.run)
    .execute(&f.pool)
    .await
    .unwrap();
    f.surprises.maintain_experiment().await.unwrap();
    let harmful = f
        .surprises
        .admin_reading_experiment(&f.digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(harmful.decision, "guardrail_harm");
    assert_eq!(harmful.state, "stopped");
    sqlx::query("DELETE FROM surprise_content WHERE id=$1")
        .bind(content)
        .execute(&f.pool)
        .await
        .unwrap();
    f.cleanup().await;
}
