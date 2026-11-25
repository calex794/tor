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

// ADAPT: independent expected scores and exact top-20 membership. In
// particular, PostgreSQL INTERSECT is distinct and the ID tiebreak is textual.
#[tokio::test]
#[ignore = "requires disposable tor_ci current model schema"]
async fn content_scores_and_textual_cutoff_are_exact_in_each_tier() {
    let mut f = fixture("contextual-score-contract").await;
    let seed = f.books[0];
    let manual = f.books[1];
    let language = format!("ctx-score-{seed}");
    let mut setup = f.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE books IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *setup)
        .await
        .unwrap();
    let maximum: i64 = sqlx::query_scalar("SELECT coalesce(max(id),0) FROM books")
        .fetch_one(&mut *setup)
        .await
        .unwrap();
    // Reserve IDs straddling a power of ten so numeric ordering would fail.
    let mut boundary = 1000_i64;
    while boundary <= maximum.checked_add(200).unwrap() {
        boundary = boundary.checked_mul(10).unwrap();
    }
    sqlx::query(
        "UPDATE books SET target_language=$2,
         genres=ARRAY['g1','g2','g3','g4','g1'],
         tags=ARRAY['t1','t2','t3','t4','t5','t6','t1'] WHERE id=$1",
    )
    .bind(seed)
    .bind(&language)
    .execute(&mut *setup)
    .await
    .unwrap();

    // Each score is pinned independently of the production SQL expression.
    type Case<'a> = (&'a [&'a str], &'a [&'a str], Option<i16>, &'a str);
    let cases: &[Case<'_>] = &[
        (
            &["g1", "g2", "g3", "g4"],
            &["t1", "t2", "t3", "t4", "t5", "t6"],
            Some(1000),
            "general",
        ),
        (&["g1", "g2", "g3", "g4"], &[], Some(550), "default"),
        (
            &[],
            &["t1", "t2", "t3", "t4", "t5", "t6"],
            Some(610),
            "default",
        ),
        (&["g1", "g1"], &["t1", "t1"], Some(310), "general"),
        (&["g1", "g1"], &[], Some(190), "default"),
        (&["g9"], &["t9"], None, "default"),
        (&[], &[], None, "default"),
        (&["g1"], &["t1"], None, "other-language"),
        (&["g1"], &["t1"], None, "held"),
        (&["g1"], &["t1"], None, "hidden"),
        (&["g1"], &["t1"], None, "pending"),
        (&["g1"], &["t1"], None, "draft"),
        (&["g1"], &["t1"], None, "unreviewed"),
        // An adult audience is eligible only for explicitly restricted tiers.
        (&["g1"], &["t1"], Some(310), "adult"),
    ];
    let mut extra = Vec::new();
    let mut expected = Vec::new();
    for (index, tier) in ["basic", "adult", "pro_adult"].into_iter().enumerate() {
        let offset = i64::try_from(index).unwrap() * 30;
        let mut rows = Vec::new();
        for n in 0..24_i64 {
            let id = if n < 12 {
                boundary - 100 + offset + n
            } else {
                boundary + offset + n - 12
            };
            let tied: Case<'_> = (&[], &["t1", "t1"], Some(130), "default");
            rows.push((id, tied));
        }
        for (n, case) in cases.iter().copied().enumerate() {
            rows.push((boundary + 100 + offset + i64::try_from(n).unwrap(), case));
        }

        let mut eligible = Vec::new();
        for (id, (genres, tags, score, state)) in rows {
            let genres: Vec<String> = genres.iter().map(|s| (*s).to_owned()).collect();
            let tags: Vec<String> = tags.iter().map(|s| (*s).to_owned()).collect();
            sqlx::query(
                "INSERT INTO books(id,source_identity,target_language,title,visibility,
                                   access_level,genres,tags,admin_hold,
                                   visibility_status,lifecycle_status)
                 VALUES($1,$2,$3,'Score contract',$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(id)
            .bind(format!("ci:contextual-score:{seed}:{id}"))
            .bind(if state == "other-language" {
                "ru"
            } else {
                language.as_str()
            })
            .bind(if state == "draft" { "draft" } else { "public" })
            .bind(tier)
            .bind(genres)
            .bind(tags)
            .bind(state == "held")
            .bind(if state == "hidden" { "hidden" } else { "active" })
            .bind(if state == "pending" {
                "pending_deletion"
            } else {
                "in_progress"
            })
            .execute(&mut *setup)
            .await
            .unwrap();
            if matches!(state, "general" | "adult" | "unreviewed") {
                sqlx::query("INSERT INTO book_recommendation_audiences(book_id,audience) VALUES($1,$2)")
                    .bind(id)
                    .bind(state)
                    .execute(&mut *setup)
                    .await
                    .unwrap();
            }
            extra.push(id);
            if state == "adult" && tier == "basic" {
                continue;
            }
            if let Some(score) = score {
                eligible.push((id, score));
            }
        }
        assert_eq!(eligible.len(), if tier == "basic" { 29 } else { 30 });
        eligible.sort_by_key(|(id, score)| (std::cmp::Reverse(*score), id.to_string()));
        for (id, score) in eligible.into_iter().take(20) {
            expected.push((
                id,
                "content".to_owned(),
                "similar_by_content".to_owned(),
                score,
            ));
        }
    }
    sqlx::query("INSERT INTO book_recommendation_relations(book_id,related_book_id) VALUES($1,$2)")
        .bind(seed)
        .bind(manual)
        .execute(&mut *setup)
        .await
        .unwrap();
    setup.commit().await.unwrap();
    f.books.extend(extra);
    expected.push((
        manual,
        "related_books".to_owned(),
        "manually_related".to_owned(),
        1000,
    ));
    assert_eq!(expected.len(), 61);
    expected.sort_unstable();

    let generation = command(&f, Action::Refresh, None).await;
    sqlx::query("UPDATE rec_generations SET cursor_book_id=$2 WHERE id=$1::uuid")
        .bind(&generation)
        .bind(seed - 1)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f.store.advance_contextual_generation().await.unwrap());
    let cursor: i64 =
        sqlx::query_scalar("SELECT cursor_book_id FROM rec_generations WHERE id=$1::uuid")
            .bind(&generation)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(cursor, seed);
    let mut actual: Vec<(i64, String, String, i16)> = sqlx::query_as(
        "SELECT book_id,source,reason,score FROM rec_candidate_edges
         WHERE generation_id=$1::uuid AND seed_id=$2",
    )
    .bind(&generation)
    .bind(seed)
    .fetch_all(&f.pool)
    .await
    .unwrap();
    actual.sort_unstable();
    assert_eq!(actual, expected);
    command(&f, Action::Cancel, Some(&generation)).await;
    stop(&f, &[generation]).await;
    f.cleanup().await;
}
