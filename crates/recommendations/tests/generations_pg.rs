//! Real PG acceptance for bounded sources, live eligibility and generation switching.
use sqlx::PgPool;
use tor_recommendations::{repositories::PgRecommendations,domain::feedback::{Choice,FeedbackError},services::feedback::new_ticket};

async fn setup()->(PgPool,PgRecommendations,i64,Vec<i64>) {
    let url=std::env::var("TEST_DATABASE_URL").expect("disposable database");
    assert!(url.ends_with("/tor_ci"));
    let pool=PgPool::connect(&url).await.unwrap();
    let actor:i64=sqlx::query_scalar("INSERT INTO users(email_normalized,display_name,password_hash) VALUES('rec13-gen@tor-ci.example','Reader','$argon2id$test') ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let mut books=Vec::new();
    for index in 0..5 {
        let book:i64=sqlx::query_scalar("INSERT INTO books(source_identity,target_language,title,visibility,genres,tags) VALUES($1,'ru',$2,'public',ARRAY['rec13-generation'],ARRAY['space']) ON CONFLICT(source_identity,target_language) DO UPDATE SET visibility='public',admin_hold=false,is_deleted=false RETURNING id")
            .bind(format!("ci:rec13:generation:{index}")).bind(format!("Generation book {index}")).fetch_one(&pool).await.unwrap();
        if index<4 {
            sqlx::query("INSERT INTO book_recommendation_audiences(book_id,audience) VALUES($1,'general') ON CONFLICT(book_id) DO UPDATE SET audience='general'")
                .bind(book).execute(&pool).await.unwrap();
        }
        books.push(book);
    }
    sqlx::query("INSERT INTO book_recommendation_relations(book_id,related_book_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(books[0]).bind(books[1]).execute(&pool).await.unwrap();
    (pool.clone(),PgRecommendations::new(pool),actor,books)
}
async fn build(store:&PgRecommendations)->String {
    let id=store.request_refresh().await.unwrap();
    for _ in 0..200 {
        if !store.advance_generation().await.unwrap() {return id;}
    }
    panic!("bounded test catalog did not finish materialization");
}
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn rec13_generation_visibility_cas_and_rollback() {
    let (pool,store,actor,books)=setup().await;
    let before=store.feed(None,Some(books[0]),20).await.unwrap();
    assert!(before.items.iter().all(|b|b.book_id!=books[4].to_string()));
    assert_eq!(store.feed(None,Some(books[4]),20).await.err(),Some(FeedbackError::NotFound));
    let generation=build(&store).await;
    let revision=store.runtime_status().await.unwrap().2;
    let activated=store.activate_generation(&generation,revision,100).await.unwrap();
    let feed=store.feed(None,Some(books[0]),20).await.unwrap();
    assert_eq!(feed.generation_id.as_deref(),Some(generation.as_str()));
    assert_eq!(feed.items[0].book_id,books[1].to_string());
    assert_eq!(feed.items[0].source,"related_books");
    assert!(feed.items.iter().all(|b|b.book_id!=books[0].to_string()&&b.book_id!=books[4].to_string()));
    assert!(feed.items.len()<=20);
    let immutable=sqlx::query("UPDATE rec_candidate_edges SET score=1 WHERE generation_id=$1::uuid")
        .bind(&generation).execute(&pool).await;
    assert!(immutable.is_err(),"validated generation edges must be immutable");
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1").bind(books[1]).execute(&pool).await.unwrap();
    let held=store.feed(None,Some(books[0]),20).await.unwrap();
    assert!(held.items.iter().all(|b|b.book_id!=books[1].to_string()));
    sqlx::query("UPDATE book_recommendation_audiences SET audience='adult' WHERE book_id=$1").bind(books[2]).execute(&pool).await.unwrap();
    assert!(store.feed(None,Some(books[0]),20).await.unwrap().items.iter().all(|b|b.book_id!=books[2].to_string()));
    let consent=store.consent(actor).await.unwrap();
    if !consent.enabled {store.set_consent(actor,true,consent.revision,&new_ticket().unwrap()).await.unwrap();}
    store.set_feedback(actor,books[3],Choice::NotInterested,0,&new_ticket().unwrap()).await.unwrap();
    assert!(store.feed(Some(actor),Some(books[0]),20).await.unwrap().items.iter().all(|b|b.book_id!=books[3].to_string()));
    assert_eq!(store.hold(revision).await,Err(FeedbackError::Conflict));
    let held_revision=store.hold(activated).await.unwrap();
    assert!(store.feed(None,None,20).await.unwrap().generation_id.is_none());
    let second=store.request_refresh().await.unwrap();
    assert_eq!(store.activate_generation(&second,held_revision,100).await,Err(FeedbackError::Conflict));
    assert_eq!(store.runtime_status().await.unwrap().0,Some(generation.clone()));
    while store.advance_generation().await.unwrap() {}
    let second_revision=store.activate_generation(&second,held_revision,100).await.unwrap();
    store.rollback(second_revision).await.unwrap();
    assert_eq!(store.runtime_status().await.unwrap().0,Some(generation));
    assert_eq!(store.feed(None,None,21).await.err(),Some(FeedbackError::Invalid));
}
