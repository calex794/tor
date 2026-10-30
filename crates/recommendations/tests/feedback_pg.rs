//! REC-13 real PostgreSQL acceptance. Only the disposable tor_ci database.
use sqlx::PgPool;
use tor_recommendations::{
    domain::feedback::{
        Choice, Feed, FeedItem, FeedbackError, ImpressionClaim, Surface, ALGORITHM,
    },
    repositories::PgRecommendations,
    services::feedback::new_ticket,
};

async fn fixture(name: &str) -> (PgPool, PgRecommendations, i64, i64) {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL required");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM users WHERE email_normalized=$1")
        .bind(format!("rec13-{name}@tor-ci.example"))
        .execute(&pool)
        .await
        .unwrap();
    let actor: i64=sqlx::query_scalar("INSERT INTO users(email_normalized,display_name,password_hash) VALUES($1,'REC reader','$argon2id$test') RETURNING id")
        .bind(format!("rec13-{name}@tor-ci.example")).fetch_one(&pool).await.unwrap();
    let book: i64=sqlx::query_scalar("INSERT INTO books(source_identity,target_language,title,visibility) VALUES($1,'ru','REC public book','public') ON CONFLICT(source_identity,target_language) DO UPDATE SET visibility='public',admin_hold=false,is_deleted=false RETURNING id")
        .bind(format!("ci:rec13:{name}")).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO book_recommendation_audiences(book_id,audience) VALUES($1,'general') ON CONFLICT(book_id) DO UPDATE SET audience='general'")
        .bind(book).execute(&pool).await.unwrap();
    let store = PgRecommendations::new(pool.clone());
    (pool, store, actor, book)
}
fn feed(book: i64) -> Feed {
    Feed {
        items: vec![FeedItem {
            book_id: book.to_string(),
            title: "REC public book".to_owned(),
            reason: "similar_by_content".to_owned(),
            source: "content".to_owned(),
            algorithm_version: ALGORITHM,
            ticket: None,
            feedback_revision: Some(0),
        }],
        generation_id: None,
        next_cursor: None,
        personalized: true,
    }
}
async fn enable(store: &PgRecommendations, actor: i64) {
    assert_eq!(
        store
            .set_consent(actor, true, 0, &new_ticket().unwrap())
            .await
            .unwrap()
            .revision,
        1
    );
}
async fn assignment(store: &PgRecommendations, actor: i64, book: i64) -> String {
    store
        .issue_tickets(actor, &[7; 32], Surface::Home, feed(book))
        .await
        .unwrap()
        .items
        .remove(0)
        .ticket
        .unwrap()
}
fn impression(ticket: &str, visible: bool) -> ImpressionClaim {
    ImpressionClaim {
        ticket: ticket.to_owned(),
        visible_fraction_per_mille: 500,
        continuous_visible_ms: 1000,
        document_visible: visible,
    }
}
async fn canonical_start(pool: &PgPool, actor: i64, book: i64) -> String {
    let chapter: i64=sqlx::query_scalar("INSERT INTO chapters(book_id,chapter_no,title,visibility) VALUES($1,1,'First','public') ON CONFLICT(book_id,chapter_no) DO UPDATE SET visibility='public' RETURNING id")
        .bind(book).fetch_one(pool).await.unwrap();
    let version: i64=sqlx::query_scalar("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES($1,1,'Readable text') ON CONFLICT(chapter_id,version_no) DO NOTHING RETURNING id")
        .bind(chapter).fetch_optional(pool).await.unwrap().unwrap_or_else(|| 0);
    let version = if version == 0 {
        sqlx::query_scalar("SELECT id FROM chapter_versions WHERE chapter_id=$1 AND version_no=1")
            .bind(chapter)
            .fetch_one(pool)
            .await
            .unwrap()
    } else {
        version
    };
    let id: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reader_chapter_starts(user_id,book_id,chapter_id,first_version_id,source_event_id) VALUES($1,$2,$3,$4,$5::uuid)")
        .bind(actor).bind(book).bind(chapter).bind(version).bind(&id).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers) VALUES($1::uuid,'ChapterStartedV1',1,'chapter',$2,$1,jsonb_build_object('user_id',$3::bigint,'book_id',$4::bigint,'chapter_id',$2::bigint,'version_id',$5::bigint,'authority','C-verified-free-start'),ARRAY['recommendation-feedback-v1'])")
        .bind(&id).bind(chapter).bind(actor).bind(book).bind(version).execute(pool).await.unwrap();
    id
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn rec13_consent_is_off_and_revocation_invalidates_tickets() {
    let (pool, store, actor, book) = fixture("consent").await;
    assert!(!store.consent(actor).await.unwrap().enabled);
    assert!(matches!(
        store
            .issue_tickets(actor, &[7; 32], Surface::Home, feed(book))
            .await,
        Err(FeedbackError::Forbidden)
    ));
    enable(&store, actor).await;
    let ticket = assignment(&store, actor, book).await;
    store.activate(actor, &[7; 32], &ticket).await.unwrap();
    store
        .set_consent(actor, false, 1, &new_ticket().unwrap())
        .await
        .unwrap();
    assert_eq!(
        store.activate(actor, &[7; 32], &ticket).await,
        Err(FeedbackError::Forbidden)
    );
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM rec_assignments a JOIN rec_subjects s ON s.subject_id=a.subject_id WHERE s.user_id=$1").bind(actor).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 0);
    enable_again(&store, actor).await;
    assert_eq!(
        store.activate(actor, &[7; 32], &ticket).await,
        Err(FeedbackError::NotFound)
    );
}
async fn enable_again(store: &PgRecommendations, actor: i64) {
    store
        .set_consent(actor, true, 2, &new_ticket().unwrap())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn rec13_impressions_and_clicks_are_deduplicated_and_session_bound() {
    let (pool, store, actor, book) = fixture("dedup").await;
    enable(&store, actor).await;
    let ticket = assignment(&store, actor, book).await;
    assert_eq!(
        store.activate(actor, &[8; 32], &ticket).await,
        Err(FeedbackError::NotFound)
    );
    store
        .impressions(actor, &[7; 32], &[impression(&ticket, false)])
        .await
        .unwrap();
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM rec_attribution_events e JOIN rec_assignments a ON a.id=e.assignment_id WHERE a.book_id=$1").bind(book).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 0);
    for _ in 0..3 {
        store
            .impressions(actor, &[7; 32], &[impression(&ticket, true)])
            .await
            .unwrap();
        assert_eq!(
            store.activate(actor, &[7; 32], &ticket).await.unwrap(),
            book
        );
    }
    store.refresh_quality().await.unwrap();
    let row = store
        .quality(1)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.book_id == book.to_string())
        .unwrap();
    assert_eq!((row.impressions, row.paired_clicks, row.starts), (1, 1, 0));
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.activate(actor, &[7; 32], &ticket).await,
        Err(FeedbackError::NotFound)
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn rec13_explicit_feedback_has_cas_and_command_replay() {
    let (_pool, store, actor, book) = fixture("choice").await;
    enable(&store, actor).await;
    let key = new_ticket().unwrap();
    let receipt = store
        .set_feedback(actor, book, Choice::NotInterested, 0, &key)
        .await
        .unwrap();
    assert!(receipt.hidden);
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::NotInterested, 0, &key)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        store
            .set_feedback(actor, book, Choice::Clear, 0, &key)
            .await,
        Err(FeedbackError::Conflict)
    );
    assert!(store
        .issue_tickets(actor, &[7; 32], Surface::Home, feed(book))
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(store.hidden_books(actor, 0).await.unwrap().len(), 1);
    store
        .set_feedback(actor, book, Choice::Clear, 1, &new_ticket().unwrap())
        .await
        .unwrap();
    assert_eq!(
        store
            .set_feedback(
                actor,
                book,
                Choice::NotInterested,
                0,
                &new_ticket().unwrap()
            )
            .await,
        Err(FeedbackError::Conflict)
    );
    assert_eq!(
        store
            .issue_tickets(actor, &[7; 32], Surface::Home, feed(book))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn rec13_real_c_event_attributes_once_to_last_click() {
    let (pool, store, actor, book) = fixture("source").await;
    enable(&store, actor).await;
    let first = assignment(&store, actor, book).await;
    store.activate(actor, &[7; 32], &first).await.unwrap();
    let last = assignment(&store, actor, book).await;
    store.activate(actor, &[7; 32], &last).await.unwrap();
    let event = canonical_start(&pool, actor, book).await;
    let (a, b) = tokio::join!(
        store.accept_read_event(&event),
        store.accept_read_event(&event)
    );
    assert_eq!(
        [a.unwrap(), b.unwrap()].into_iter().filter(|x| *x).count(),
        1
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rec_first_conversions WHERE source_event_id=$1::uuid",
    )
    .bind(&event)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let facts:i64=sqlx::query_scalar("SELECT count(*) FROM rec_attribution_events WHERE source_event_id=$1::uuid AND trust='domain_committed'").bind(&event).fetch_one(&pool).await.unwrap();
    assert_eq!(facts, 1);
    store.refresh_quality().await.unwrap();
    let rows = store.quality(1).await.unwrap();
    assert_eq!(
        rows.iter()
            .filter(|r| r.book_id == book.to_string())
            .map(|r| r.starts)
            .sum::<i64>(),
        1
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn rec13_forged_payload_without_c_fact_is_rejected() {
    let (pool, store, actor, book) = fixture("forged").await;
    enable(&store, actor).await;
    let id:String=sqlx::query_scalar("INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers) VALUES('ChapterCompletedV1',1,'chapter',1,gen_random_uuid()::text,jsonb_build_object('user_id',$1::bigint,'book_id',$2::bigint,'trusted',true,'authority','C-verified-free-position'),ARRAY['recommendation-feedback-v1']) RETURNING id::text")
        .bind(actor).bind(book).fetch_one(&pool).await.unwrap();
    assert_eq!(
        store.accept_read_event(&id).await,
        Err(FeedbackError::Forbidden)
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rec_first_conversions WHERE source_event_id=$1::uuid",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
}
