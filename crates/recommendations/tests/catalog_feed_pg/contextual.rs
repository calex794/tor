//! Current schema, genuine metadata builder and live serving; no fabricated graph.
use super::*;
use tor_recommendations::services::contextual::{Action, Control, MODEL};

pub(super) async fn command(f: &Fixture, action: Action, generation: Option<&str>) -> String {
    let revision = f.store.contextual_status().await.unwrap().revision;
    f.store
        .control_contextual(&Control {
            action,
            expected_revision: revision,
            generation: generation.map(str::to_owned),
        })
        .await
        .unwrap()
        .generation
        .unwrap_or_default()
}
async fn build(f: &Fixture) -> String {
    let id = command(f, Action::Refresh, None).await;
    for _ in 0..2000 {
        assert!(f.store.advance_contextual_generation().await.unwrap());
        let state: String =
            sqlx::query_scalar("SELECT status FROM rec_generations WHERE id=$1::uuid")
                .bind(&id)
                .fetch_one(&f.pool)
                .await
                .unwrap();
        if state == "validated" {
            return id;
        }
    }
    panic!("bounded generation did not finish");
}
pub(super) async fn stop(f: &Fixture, ids: &[String]) {
    sqlx::query("UPDATE rec_runtime_policy SET active_generation=NULL,previous_generation=NULL,rollout_percent=0,hold=true,revision=revision+1 WHERE singleton")
      .execute(&f.pool).await.unwrap();
    for id in ids {
        sqlx::query("DELETE FROM rec_generations WHERE id=$1::uuid")
            .bind(id)
            .execute(&f.pool)
            .await
            .unwrap();
    }
}
pub(super) async fn metadata(f: &Fixture) {
    // Unique language ensures an isolated cohort even when other acceptance fixtures remain.
    sqlx::query("UPDATE books SET target_language=$2,genres=ARRAY['shared'],tags=ARRAY['system'] WHERE id=ANY($1::bigint[])")
       .bind(&f.books).bind(format!("ctx-{}",f.books[0])).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO book_recommendation_relations(book_id,related_book_id) VALUES($1,$2)")
        .bind(f.books[0])
        .bind(f.books[1])
        .execute(&f.pool)
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "requires disposable tor_ci current model schema"]
async fn actual_metadata_model_preserves_choices_live_rights_and_rollback_boundaries() {
    let f = fixture("contextual").await;
    metadata(&f).await;
    let original = f.store.contextual_status().await.unwrap();
    assert!(original.held);
    let first = build(&f).await;
    assert_catalog(&f.store.feed(None, Some(f.books[0]), 20).await.unwrap());
    command(&f, Action::Activate, Some(&first)).await;
    let baseline = f.store.feed(None, Some(f.books[0]), 20).await.unwrap();
    assert_eq!(baseline.generation_id.as_deref(), Some(first.as_str()));
    assert_eq!(ids(&baseline).len(), 4);
    assert_eq!(ids(&baseline)[0], f.books[1]);
    assert_eq!(baseline.items[0].reason, "manually_related");
    assert!(baseline
        .items
        .iter()
        .all(|i| i.algorithm_version == MODEL && i.ticket.is_none()));
    assert!(!baseline.personalized);
    assert_eq!(
        ids(&f
            .store
            .feed(Some(f.actors[1]), Some(f.books[0]), 20)
            .await
            .unwrap()),
        ids(&baseline)
    );
    let mut history = f.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE rec_subjects,rec_reader_signals,rec_assignments,rec_quality_daily,organic_discovery_quality_daily IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *history).await.unwrap();
    let read = tokio::time::timeout(
        Duration::from_secs(1),
        f.store.feed(None, Some(f.books[0]), 20),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(ids(&read), ids(&baseline));
    history.rollback().await.unwrap();
    // Real read-only role configuration proves GET never creates assignments/preferences.
    let readonly = PgPoolOptions::new()
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_read_only=on")
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&current_analytics::url("TEST_DATABASE_URL", "tor_app"))
        .await
        .unwrap();
    let read = PgRecommendations::new(readonly.clone())
        .feed(Some(f.actors[0]), Some(f.books[0]), 20)
        .await
        .unwrap();
    assert_eq!(ids(&read), ids(&baseline));
    readonly.close().await;
    f.store
        .set_feedback(
            f.actors[0],
            f.books[1],
            Choice::NotInterested,
            0,
            &new_ticket().unwrap(),
        )
        .await
        .unwrap();
    assert!(!ids(&f
        .store
        .feed(Some(f.actors[0]), Some(f.books[0]), 20)
        .await
        .unwrap())
    .contains(&f.books[1]));
    assert!(ids(&f
        .store
        .feed(Some(f.actors[1]), Some(f.books[0]), 20)
        .await
        .unwrap())
    .contains(&f.books[1]));
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(f.books[2])
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(!ids(&f.store.feed(None, Some(f.books[0]), 20).await.unwrap()).contains(&f.books[2]));
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(f.books[3])
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(!ids(&f.store.feed(None, Some(f.books[0]), 20).await.unwrap()).contains(&f.books[3]));
    sqlx::query("UPDATE users SET adult_enabled=true WHERE id=$1")
        .bind(f.actors[1])
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(ids(&f
        .store
        .feed(Some(f.actors[1]), Some(f.books[0]), 20)
        .await
        .unwrap())
    .contains(&f.books[3]));
    sqlx::query("DELETE FROM book_recommendation_relations WHERE book_id=$1")
        .bind(f.books[0])
        .execute(&f.pool)
        .await
        .unwrap();
    let after = f.store.feed(None, Some(f.books[0]), 20).await.unwrap();
    assert!(after.items.iter().all(|i| i.reason == "similar_by_content"));
    let second = build(&f).await;
    command(&f, Action::Activate, Some(&second)).await;
    command(&f, Action::Rollback, None).await;
    assert_eq!(
        f.store
            .contextual_status()
            .await
            .unwrap()
            .active_generation
            .as_deref(),
        Some(first.as_str())
    );
    command(&f, Action::Hold, None).await;
    assert_catalog(&f.store.feed(None, Some(f.books[0]), 20).await.unwrap());
    command(&f, Action::Activate, Some(&second)).await;
    assert_catalog(&f.store.feed(None, None, 20).await.unwrap());
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(f.books[0])
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.store.feed(None, Some(f.books[0]), 20).await.err(),
        Some(FeedbackError::NotFound)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rec_subjects WHERE user_id=ANY($1::bigint[])"
        )
        .bind(f.actors.as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap(),
        0
    );
    stop(&f, &[first, second]).await;
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci current model schema"]
async fn contextual_admin_cas_cancellation_and_edge_fences_are_atomic() {
    let f = fixture("contextual-cas").await;
    metadata(&f).await;
    let revision = f.store.contextual_status().await.unwrap().revision;
    let control = Control {
        action: Action::Refresh,
        expected_revision: revision,
        generation: None,
    };
    let (a, b) = tokio::join!(
        f.store.control_contextual(&control),
        f.store.control_contextual(&control)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let id = a.or(b).unwrap().generation.unwrap();
    assert_eq!(
        command(&f, Action::Refresh, None).await,
        id,
        "refresh coalesces one unfinished build"
    );
    for (source, reason, seed) in [
        ("behavioral", "readers_also_read", f.books[0]),
        ("popularity", "popular", 0),
        ("content", "similar_by_content", 0),
    ] {
        let e=sqlx::query("INSERT INTO rec_candidate_edges(generation_id,seed_id,book_id,source,reason,score) VALUES($1::uuid,$2,$3,$4,$5,1000)")
          .bind(&id).bind(seed).bind(f.books[1]).bind(source).bind(reason).execute(&f.pool).await.unwrap_err();
        assert_eq!(
            e.as_database_error().unwrap().code().as_deref(),
            Some("23514")
        );
    }
    // A real edge writer holds parent SHARE. Finalization cannot overtake it.
    let mut writer = f.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO rec_candidate_edges(generation_id,seed_id,book_id,source,reason,score) VALUES($1::uuid,$2,$3,'content','similar_by_content',310)")
       .bind(&id).bind(f.books[0]).bind(f.books[1]).execute(&mut *writer).await.unwrap();
    let mut finalizer = f.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='50ms'")
        .execute(&mut *finalizer)
        .await
        .unwrap();
    let result=sqlx::query("UPDATE rec_generations SET status='validated',finished_at=now(),manifest_hash=decode(repeat('ab',32),'hex') WHERE id=$1::uuid")
       .bind(&id).execute(&mut *finalizer).await.unwrap_err();
    assert_eq!(
        result.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    finalizer.rollback().await.unwrap();
    writer.rollback().await.unwrap();
    command(&f, Action::Cancel, Some(&id)).await;
    assert!(!f.store.advance_contextual_generation().await.unwrap());
    let bad = Control {
        action: Action::Activate,
        expected_revision: f.store.contextual_status().await.unwrap().revision,
        generation: Some(id.clone()),
    };
    assert_eq!(
        f.store.control_contextual(&bad).await.err(),
        Some(FeedbackError::Conflict)
    );
    let second = build(&f).await;
    let immutable =
        sqlx::query("UPDATE rec_candidate_edges SET score=1 WHERE generation_id=$1::uuid")
            .bind(&second)
            .execute(&f.pool)
            .await
            .unwrap_err();
    assert_eq!(
        immutable.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    let version = sqlx::query(
        "UPDATE rec_generations SET model_version='legacy_feedback_v1' WHERE id=$1::uuid",
    )
    .bind(&second)
    .execute(&f.pool)
    .await
    .unwrap_err();
    assert_eq!(
        version.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    stop(&f, &[id, second]).await;
    f.cleanup().await;
}
