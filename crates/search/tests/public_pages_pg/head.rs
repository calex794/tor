use super::*;
use axum::{extract::State,http::{HeaderMap,Method,StatusCode,Uri},response::IntoResponse,Router};
use std::sync::{atomic::{AtomicBool,AtomicU16,AtomicU64,AtomicUsize,Ordering},Arc,Mutex};
use tor_search::services::public_pages::PublicPageProbe;
struct Server {
    status:AtomicU16,
    withdraw:AtomicBool,
    delay_ms:AtomicU64,
    pool:sqlx::PgPool,
    active:AtomicUsize,
    max_active:AtomicUsize,
    requests:Mutex<Vec<(Method,String,bool,bool,String)>>,
}
async fn answer(State(state):State<Arc<Server>>,method:Method,uri:Uri,headers:HeaderMap)->impl IntoResponse {
    let active=state.active.fetch_add(1,Ordering::SeqCst)+1;
    state.max_active.fetch_max(active,Ordering::SeqCst);
    state.requests.lock().unwrap().push((method,uri.path().into(),
        headers.contains_key("cookie"),headers.contains_key("authorization"),
        headers.get("user-agent").and_then(|h|h.to_str().ok()).unwrap_or("").into()));
    if uri.path()=="/books/1" && state.withdraw.swap(false,Ordering::SeqCst) {
        sqlx::query("UPDATE books SET admin_hold=true WHERE id=1").execute(&state.pool).await.unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_millis(state.delay_ms.load(Ordering::SeqCst))).await;
    state.active.fetch_sub(1,Ordering::SeqCst);
    (StatusCode::from_u16(state.status.load(Ordering::SeqCst)).unwrap(),
        [("location","/must-not-follow")],"NO READER CONTENT")
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
#[ignore="requires disposable PostgreSQL 18 and current migrations"]
async fn real_head_uses_only_four_current_targets_no_credentials_or_redirects() {
    let f=Fixture::new().await;
    let state=Arc::new(Server {status:AtomicU16::new(200),withdraw:AtomicBool::new(false),delay_ms:AtomicU64::new(40),pool:f.pool.clone(),active:AtomicUsize::new(0),
        max_active:AtomicUsize::new(0),requests:Mutex::new(vec![])});
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin=format!("http://{}",listener.local_addr().unwrap());
    let app=Router::new().fallback(answer).with_state(state.clone());
    let server=tokio::spawn(async move { axum::serve(listener,app).await.unwrap(); });
    let probe=PublicPageProbe::new(&origin).unwrap();
    assert!(probe.check_once(&f.store).await.unwrap());
    assert_eq!(f.store.report(probe.origin()).await.unwrap().checked_targets(),4);
    assert!(!probe.check_once(&f.store).await.unwrap(),"a second process cannot bypass the one-minute claim");
    {
        let requests=state.requests.lock().unwrap();
        assert_eq!(requests.len(),4);
        let mut paths=requests.iter().map(|r|r.1.as_str()).collect::<Vec<_>>();paths.sort_unstable();
        assert_eq!(paths,vec!["/","/books/1","/catalog","/reader/1"]);
        assert!(requests.iter().all(|r|r.0==Method::HEAD && !r.2 && !r.3 && r.4=="TOR-AvailabilityBot/1.0"));
    }
    assert!(state.max_active.load(Ordering::SeqCst)<=2);
    state.status.store(302,Ordering::SeqCst);f.due().await;
    assert!(probe.check_once(&f.store).await.unwrap());
    assert_eq!(f.store.report(probe.origin()).await.unwrap().lost_targets(),4);
    assert!(!state.requests.lock().unwrap().iter().any(|r|r.1=="/must-not-follow"));
    server.abort();let _=server.await;f.due().await;
    assert!(probe.check_once(&f.store).await.unwrap());
    assert_eq!(f.store.report(probe.origin()).await.unwrap().checked_targets(),0);
    assert_eq!(f.store.report(probe.origin()).await.unwrap().lost_targets(),0);
    f.cleanup().await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
#[ignore="requires disposable PostgreSQL 18 and current migrations"]
async fn real_head_cannot_preserve_a_loss_after_policy_changes_during_response() {
    let f=Fixture::new().await;
    let state=Arc::new(Server {status:AtomicU16::new(200),withdraw:AtomicBool::new(false),delay_ms:AtomicU64::new(40),
        pool:f.pool.clone(),active:AtomicUsize::new(0),max_active:AtomicUsize::new(0),requests:Mutex::new(vec![])});
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin=format!("http://{}",listener.local_addr().unwrap());
    let app=Router::new().fallback(answer).with_state(state.clone());
    let server=tokio::spawn(async move { axum::serve(listener,app).await.unwrap(); });
    let probe=PublicPageProbe::new(&origin).unwrap();
    probe.check_once(&f.store).await.unwrap();
    assert_eq!(f.store.report(probe.origin()).await.unwrap().checked_targets(),4);
    state.withdraw.store(true,Ordering::SeqCst);f.due().await;
    probe.check_once(&f.store).await.unwrap();
    let report=f.store.report(probe.origin()).await.unwrap();
    assert_eq!((report.expected_targets(),report.checked_targets(),report.lost_targets()),(2,2,0));
    assert!(report.rows[2..].iter().all(|r|r.state==PageState::NoTarget && r.path.is_none()));
    server.abort();let _=server.await;drop(probe);drop(state);f.cleanup().await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
#[ignore="requires disposable PostgreSQL 18 and current migrations"]
async fn slow_head_is_unknown_bounded_and_holds_no_database_connection() {
    let f=Fixture::with_connections(1).await;
    let state=Arc::new(Server {status:AtomicU16::new(200),withdraw:AtomicBool::new(false),
        delay_ms:AtomicU64::new(40),pool:f.pool.clone(),active:AtomicUsize::new(0),
        max_active:AtomicUsize::new(0),requests:Mutex::new(vec![])});
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin=format!("http://{}",listener.local_addr().unwrap());
    let app=Router::new().fallback(answer).with_state(state.clone());
    let server=tokio::spawn(async move { axum::serve(listener,app).await.unwrap(); });
    let probe=PublicPageProbe::new(&origin).unwrap();
    assert!(probe.check_once(&f.store).await.unwrap());
    assert_eq!(f.store.report(probe.origin()).await.unwrap().checked_targets(),4);

    state.delay_ms.store(1200,Ordering::SeqCst);
    state.requests.lock().unwrap().clear();
    f.due().await;
    let started=std::time::Instant::now();
    let checking={
        let probe=probe.clone();
        let store=f.store.clone();
        tokio::spawn(async move { probe.check_once(&store).await })
    };
    tokio::time::timeout(std::time::Duration::from_millis(500),async {
        while state.active.load(Ordering::SeqCst)==0 {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    }).await.expect("the real HEAD must be in flight");
    let answer=tokio::time::timeout(std::time::Duration::from_millis(250),
        sqlx::query_scalar::<_,i32>("SELECT 42").fetch_one(&f.pool))
        .await.expect("HEAD held the only database connection").unwrap();
    assert_eq!(answer,42);
    assert!(checking.await.unwrap().unwrap());
    assert!(started.elapsed()<std::time::Duration::from_secs(4),"whole round must stay bounded");
    let report=f.store.report(probe.origin()).await.unwrap();
    assert_eq!((report.checked_targets(),report.lost_targets()),(0,0));
    assert!(report.rows.iter().all(|row|row.state==PageState::Unknown && row.status.is_none()));
    assert_eq!(state.requests.lock().unwrap().len(),4);
    server.abort();let _=server.await;drop(probe);drop(state);f.cleanup().await;
}
