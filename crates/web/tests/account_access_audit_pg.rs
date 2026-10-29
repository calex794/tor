//! Verified login IP audit on disposable PostgreSQL.
use axum::{body::{to_bytes,Body},extract::ConnectInfo,http::{header,Request,StatusCode},Router};
use std::{net::SocketAddr,time::{SystemTime,UNIX_EPOCH}};
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;
const ORIGIN:&str="http://127.0.0.1:3098";
fn request(method:&str,path:&str,body:&str,cookies:&str,ip:[u8;4],ua:&str)->Request<Body>{
 let mut req=Request::builder().method(method).uri(path)
 .header(header::CONTENT_TYPE,"application/x-www-form-urlencoded")
 .header(header::ORIGIN,ORIGIN).header(header::COOKIE,cookies)
 .header(header::USER_AGENT,ua).header("x-forwarded-for","192.0.2.99")
 .body(Body::from(body.to_owned())).unwrap();
 req.extensions_mut().insert(ConnectInfo(SocketAddr::from((ip,40001))));
 req
}
async fn send(app:&Router,req:Request<Body>)->axum::response::Response{
 app.clone().oneshot(req).await.unwrap()
}
fn cookie(resp:&axum::response::Response,prefix:&str)->String{
 resp.headers().get_all(header::SET_COOKIE).iter().filter_map(|v|v.to_str().ok())
 .find(|v|v.starts_with(prefix)).unwrap().split(';').next().unwrap().to_owned()
}
async fn csrf(app:&Router,path:&str)->(String,String){
 let page=send(app,request("GET",path,"","",[198,51,100,3],"Test/1")).await;
 assert_eq!(page.status(),StatusCode::OK);
 let c=cookie(&page,"tor-dev-csrf=");
 let html=to_bytes(page.into_body(),128000).await.unwrap();
 let s=String::from_utf8(html.to_vec()).unwrap();
 let token=s.split("name=\"csrf_token\" value=\"").nth(1).unwrap().split('"').next().unwrap().to_owned();
 (c,token)
}
#[tokio::test]
#[ignore="requires disposable PostgreSQL tor_ci"]
async fn access_audit_links_two_sessions_to_distinct_verified_ips(){
 let url=std::env::var("TEST_DATABASE_URL").unwrap();
 assert!(url.ends_with("/tor_ci"),"production forbidden");
 let catalog=PgCatalog::connect(&url).await.unwrap();
 let pool=catalog.pool().clone();
 let reader=PgReader::new(pool.clone());
 let account=PgAccounts::from_pool(pool.clone());
 account.probe().await.unwrap();
 let app=tor_web::router_with_postgres_reading_auth(catalog,reader,account,false,ORIGIN.to_owned());
 let nonce=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
 let email=format!("audit-{nonce}@example.invalid");
 let (c,t)=csrf(&app,"/register").await;
 let form=format!("email=audit-{nonce}%40example.invalid&display_name=AuditReader&password=LongSecurePassword2026&csrf_token={t}");
 let r=send(&app,request("POST","/register",&form,&c,[198,51,100,12],"Chrome/120 Safari/537")).await;
 assert_eq!(r.status(),StatusCode::SEE_OTHER);
 let first=cookie(&r,"tor-dev-session=");
 let user:i64=sqlx::query_scalar("SELECT id FROM users WHERE email_normalized=$1")
 .bind(&email).fetch_one(&pool).await.unwrap();
 let (c,t)=csrf(&app,"/login").await;
 let bad=format!("email=audit-{nonce}%40example.invalid&password=wrong&csrf_token={t}");
 let r=send(&app,request("POST","/login",&bad,&c,[203,0,113,41],"Firefox/120")).await;
 assert_eq!(r.status(),StatusCode::UNAUTHORIZED);
 let count:i64=sqlx::query_scalar("SELECT count(*) FROM account_login_events WHERE user_id=$1")
 .bind(user).fetch_one(&pool).await.unwrap();
 assert_eq!(count,1);
 let good=format!("email=audit-{nonce}%40example.invalid&password=LongSecurePassword2026&csrf_token={t}");
 let r=send(&app,request("POST","/login",&good,&c,[203,0,113,41],"Firefox/120")).await;
 assert_eq!(r.status(),StatusCode::SEE_OTHER);
 let second=cookie(&r,"tor-dev-session=");
 assert_ne!(first,second);
 let rows:Vec<(String,Option<String>,String,String)>=sqlx::query_as(
 "SELECT e.session_id::text,host(e.verified_ip),e.agent_family,e.event_kind
 FROM account_login_events e JOIN sessions s ON s.session_id=e.session_id
 AND s.user_id=e.user_id WHERE e.user_id=$1"
 ).bind(user).fetch_all(&pool).await.unwrap();
 assert_eq!(rows.len(),2);
 assert_ne!(rows[0].0,rows[1].0);
 assert!(rows.iter().any(|r|r.1.as_deref()==Some("198.51.100.12")&&r.2=="chrome"&&r.3=="registration"));
 assert!(rows.iter().any(|r|r.1.as_deref()==Some("203.0.113.41")&&r.2=="firefox"&&r.3=="password_login"));
 assert!(!rows.iter().any(|r|r.1.as_deref()==Some("192.0.2.99")));
 for c in [first,second]{
  let page=send(&app,request("GET","/me","",&c,[192,0,2,13],"Test/1")).await;
  assert_eq!(page.status(),StatusCode::OK);
 }
}
