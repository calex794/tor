//! F-05 browser/JSON actions on a disposable real PostgreSQL database.
use super::*;
use sqlx::PgPool;
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_recommendations::{
    repositories::{PgRanking, RankingCommand},
    services::ranking::RankingSnapshot,
};

async fn fixture(name: &str) -> (Router, PgPool, i64, [i64; 2], String) {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG URL");
    assert!(url.ends_with("/tor_ci"), "never use VPS");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let reader = PgReader::new(pool.clone());
    let email = format!("f05-api-{name}@tor-ci.example");
    sqlx::query("DELETE FROM users WHERE email_normalized=$1")
        .bind(&email).execute(&pool).await.unwrap();
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'F05 API','$argon2id$test') RETURNING id",
    ).bind(&email).fetch_one(&pool).await.unwrap();
    let token = generate_session_token();
    sqlx::query("INSERT INTO sessions(token_hash,user_id,expires_at)
                 VALUES($1,$2,now()+interval '1 day')")
        .bind(session_token_digest(&token).to_vec())
        .bind(uid).execute(&pool).await.unwrap();
    let mut ids = [0_i64; 2];
    for (n, id) in ids.iter_mut().enumerate() {
        *id = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES($1,'ru',$2,'public')
             ON CONFLICT(source_identity,target_language) DO UPDATE
             SET title=EXCLUDED.title,visibility='public',
                 admin_hold=false,is_deleted=false RETURNING id",
        ).bind(format!("ci:f05:api:{name}:{n}"))
            .bind(format!("Проверенная книга {n}"))
            .fetch_one(&pool).await.unwrap();
    }
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,reader,accounts,false,ORIGIN.to_owned(),
    );
    (app,pool,uid,ids,format!("tor-dev-session={token}"))
}

async fn ready(pool: &PgPool, uid: i64, ids: [i64; 2]) -> RankingSnapshot {
    // Internal test seed only; user-supplied "completed" claims are forbidden.
    let repo = PgRanking::new(pool.clone());
    let first = repo.apply(uid,0,RankingCommand::StartVerified(ids[0]))
        .await.unwrap();
    repo.apply(uid,first.snapshot.revision,RankingCommand::StartVerified(ids[1]))
        .await.unwrap().snapshot
}

async fn csrf(app: &Router, session: &str) -> (String,String) {
    let page = send(app, request("GET","/me/rankings","",session,ORIGIN)).await;
    assert_eq!(page.status(),StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL],"no-store");
    let cookie = cookie(&page,"tor-dev-csrf=");
    assert!(body_of(page).await.contains("Мой рейтинг"));
    let token = cookie.split_once('=').unwrap().1.to_owned();
    (cookie,token)
}

#[tokio::test]
#[ignore = "requires isolated tor_ci"]
async fn f_05_http_private_csrf_and_duplicate_fencing() {
    let (app,pool,uid,ids,session)=fixture("choice").await;
    let guest=send(&app,request("GET","/api/v1/me/rankings","","",ORIGIN)).await;
    assert_eq!(guest.status(),StatusCode::UNAUTHORIZED);
    let empty=send(&app,request("GET","/api/v1/me/rankings","",&session,ORIGIN)).await;
    assert!(body_of(empty).await.contains("awaiting_verified_completion"));
    let before=ready(&pool,uid,ids).await;
    let q=before.next_question().unwrap().unwrap();
    let (csrf_cookie,token)=csrf(&app,&session).await;
    let cookies=format!("{session}; {csrf_cookie}");
    let read=send(&app,request("GET","/api/v1/me/rankings","",&cookies,ORIGIN)).await;
    assert_eq!(read.status(),StatusCode::OK);
    let body:serde_json::Value=serde_json::from_str(&body_of(read).await).unwrap();
    assert_eq!(body["state"],"choose");
    assert_eq!(body["question"]["comparison_id"],q.comparison_id);
    let json=format!(
        r#"{{"expected_revision":{},"expected_state_version":{},"comparison_id":"{}","outcome":"tie"}}"#,
        before.revision,q.state_version,q.comparison_id,
    );
    for (origin,csrf) in [(ORIGIN,"bad-token"),("https://evil.example",token.as_str())] {
        let denied=send(&app,json_request(
            "POST","/api/v1/me/rankings/choice",&json,&cookies,origin,csrf,
        )).await;
        assert_eq!(denied.status(),StatusCode::FORBIDDEN);
    }
    let accepted=send(&app,json_request(
        "POST","/api/v1/me/rankings/choice",&json,&cookies,ORIGIN,&token,
    )).await;
    assert_eq!(accepted.status(),StatusCode::OK);
    assert!(body_of(accepted).await.contains(r#""status":"saved""#));
    let retry=send(&app,json_request(
        "POST","/api/v1/me/rankings/choice",&json,&cookies,ORIGIN,&token,
    )).await;
    assert_eq!(retry.status(),StatusCode::CONFLICT);
    let count:i64=sqlx::query_scalar(
        "SELECT count(*) FROM user_book_comparison WHERE user_id=$1",
    ).bind(uid).fetch_one(&pool).await.unwrap();
    assert_eq!(count,1);
    for path in ["/api/v1/me/rankings/start","/api/v1/me/rankings/completed"] {
        let response=send(&app,request("POST",path,"",&cookies,ORIGIN)).await;
        assert_eq!(response.status(),StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
#[ignore = "requires isolated tor_ci"]
async fn f_05_http_form_skip_resume_and_public_revocation() {
    let (app,pool,uid,ids,session)=fixture("skip").await;
    let before=ready(&pool,uid,ids).await;
    let q=before.next_question().unwrap().unwrap();
    let (csrf_cookie,token)=csrf(&app,&session).await;
    let cookies=format!("{session}; {csrf_cookie}");
    let page=send(&app,request("GET","/me/rankings","",&cookies,ORIGIN)).await;
    let html=body_of(page).await;
    assert!(html.contains("Первая лучше") && html.contains("Отложить вторую"));
    assert!(!html.contains("<script"));
    let form=format!(
        "csrf_token={token}&expected_revision={}&expected_state_version={}&comparison_id={}&outcome=skip",
        before.revision,q.state_version,q.comparison_id,
    );
    let skipped=send(&app,request(
        "POST","/me/rankings/choice",&form,&cookies,ORIGIN,
    )).await;
    assert_eq!(skipped.status(),StatusCode::SEE_OTHER);
    let after=PgRanking::new(pool.clone()).snapshot(uid).await.unwrap();
    assert!(after.next_question().unwrap().is_none());
    let page=send(&app,request("GET","/me/rankings","",&cookies,ORIGIN)).await;
    assert!(body_of(page).await.contains("Все предложенные пары пропущены"));
    let form=format!("csrf_token={token}&expected_revision={}",after.revision);
    let resumed=send(&app,request(
        "POST","/me/rankings/resume",&form,&cookies,ORIGIN,
    )).await;
    assert_eq!(resumed.status(),StatusCode::SEE_OTHER);
    let current=PgRanking::new(pool.clone()).snapshot(uid).await.unwrap();
    assert_ne!(q.comparison_id,current.next_question().unwrap().unwrap().comparison_id);
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(ids[0]).execute(&pool).await.unwrap();
    let hidden=send(&app,request("GET","/api/v1/me/rankings","",&cookies,ORIGIN)).await;
    assert_eq!(hidden.status(),StatusCode::CONFLICT);
    assert!(body_of(hidden).await.is_empty());
    let q=current.next_question().unwrap().unwrap();
    let json=format!(
        r#"{{"expected_revision":{},"expected_state_version":{},"comparison_id":"{}","outcome":"a_wins"}}"#,
        current.revision,q.state_version,q.comparison_id,
    );
    let rejected=send(&app,json_request(
        "POST","/api/v1/me/rankings/choice",&json,&cookies,ORIGIN,&token,
    )).await;
    assert_eq!(rejected.status(),StatusCode::CONFLICT);
    assert_eq!(PgRanking::new(pool.clone()).snapshot(uid).await.unwrap(),current);
}
