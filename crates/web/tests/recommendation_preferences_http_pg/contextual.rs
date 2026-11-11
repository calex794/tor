//! Administrator commands remain role/CSRF bound even while reader serving is disabled.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
};
use tor_books::repositories::PgCatalog;
use tor_recommendations::repositories::PgRecommendations;
const PAGE: &str = "/me/recommendations/contextual";
const API: &str = "/api/v1/admin/recommendations/contextual";
async fn post(h: &Harness, body: Value, origin: &str, csrf: &str) -> Response {
    h.send(
        "POST",
        API,
        &h.actor.cookie,
        (origin, csrf),
        &body.to_string(),
        Some("transport-only"),
    )
    .await
}
async fn status(h: &Harness) -> Value {
    let response = h.get(&h.actor, API).await;
    private(&response, StatusCode::OK);
    json(response).await
}
fn payload(action: &str, revision: i64, generation: Option<&str>) -> Value {
    json!({"action":action,"expected_revision":revision,"generation":generation})
}
struct Lock(Child, Option<ChildStdin>);
impl Lock {
    fn new(url: &str) -> Self {
        let mut child = Command::new("psql")
            .args(["-XAtq", "-v", "ON_ERROR_STOP=1", "--dbname", url])
            .env(
                "PGOPTIONS",
                "-c statement_timeout=5000 -c idle_in_transaction_session_timeout=10000",
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(
            stdin,
            "BEGIN; LOCK TABLE rec_runtime_policy IN ACCESS EXCLUSIVE MODE; SELECT 'HELD';"
        )
        .unwrap();
        stdin.flush().unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line.trim() == "HELD" {
                break;
            }
        }
        Self(child, Some(stdin))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        if let Some(mut input) = self.1.take() {
            let _ = writeln!(input, "ROLLBACK;\n\\q");
            let _ = input.flush();
        }
        let _ = self.0.wait();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL current schema"]
async fn contextual_admin_has_live_role_csrf_cas_readonly_and_outage_recovery() {
    let h = Harness::new(false).await;
    let snapshot = optional_snapshot(&h.url, h.actor.id);
    assert!(
        h.sql("SELECT count(*) FROM account_roles WHERE role='administrator' AND is_active")
            .parse::<i64>()
            .unwrap()
            > 0,
        "existing independent fixture administrator required"
    );
    for path in [PAGE, API] {
        private(&h.get(&h.actor, path).await, StatusCode::FORBIDDEN);
        private(
            &h.send("GET", path, "", (ORIGIN, ""), "", None).await,
            StatusCode::UNAUTHORIZED,
        );
    }
    h.sql(&format!(
        "INSERT INTO account_roles(user_id,role) VALUES({},'administrator')",
        h.actor.id
    ));
    let rev = status(&h).await["revision"].as_i64().unwrap();
    let page = h.get(&h.actor, PAGE).await;
    private(&page, StatusCode::OK);
    assert!(page.headers().contains_key("content-security-policy"));
    let html = text(page).await;
    assert!(html.contains("contextual-product-disabled"));
    assert!(html.contains("metadata_manual_v1"));
    assert!(!html.contains("name=\"percent\"") && !html.contains("flow-consent"));
    private(
        &post(
            &h,
            payload("refresh", rev, None),
            "https://untrusted.invalid",
            &h.actor.csrf,
        )
        .await,
        StatusCode::FORBIDDEN,
    );
    private(
        &post(&h, payload("refresh", rev, None), ORIGIN, "incorrect").await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(status(&h).await["revision"], rev);
    let response = post(&h, payload("refresh", rev, None), ORIGIN, &h.actor.csrf).await;
    private(&response, StatusCode::OK);
    let receipt = json(response).await;
    let id = receipt["generation"].as_str().unwrap().to_owned();
    let model = status(&h).await;
    let entry = &model["generations"][0];
    assert_eq!(entry["cursor_book_id"], "0");
    let created = entry["created_at"].as_str().unwrap();
    assert!(created.contains('T') && created.ends_with('Z'));

    private(
        &post(&h, payload("refresh", rev, None), ORIGIN, &h.actor.csrf).await,
        StatusCode::CONFLICT,
    );
    private(
        &post(
            &h,
            payload("activate", rev + 1, Some(&id)),
            ORIGIN,
            &h.actor.csrf,
        )
        .await,
        StatusCode::CONFLICT,
    );
    let catalog = PgCatalog::connect(&h.url).await.unwrap();
    let store = PgRecommendations::new(catalog.pool().clone());
    for _ in 0..2000 {
        store.advance_contextual_generation().await.unwrap();
        if h.sql(&format!(
            "SELECT status FROM rec_generations WHERE id='{id}'"
        )) == "validated"
        {
            break;
        }
    }
    assert_eq!(
        h.sql(&format!(
            "SELECT status FROM rec_generations WHERE id='{id}'"
        )),
        "validated"
    );
    let form = format!(
        "action=activate&generation={id}&expected_revision={}&csrf_token={}",
        rev + 1,
        h.actor.csrf
    );
    private(&h.form(&h.actor, PAGE, &form).await, StatusCode::SEE_OTHER);
    assert_eq!(status(&h).await["active_generation"], id);
    assert_eq!(
        h.get(&h.actor, "/api/v1/recommendations").await.status(),
        StatusCode::NOT_FOUND,
        "administrative activation does not enable the product flag"
    );
    let held = Lock::new(&h.url);
    let response = h.get(&h.actor, PAGE).await;
    private(&response, StatusCode::SERVICE_UNAVAILABLE);
    let html = text(response).await;
    assert!(html.contains("contextual-unavailable"));
    assert!(!html.contains("data-action=\"refresh\""));
    private(
        &post(&h, payload("hold", rev + 2, None), ORIGIN, &h.actor.csrf).await,
        StatusCode::SERVICE_UNAVAILABLE,
    );
    drop(held);
    assert_eq!(
        status(&h).await["revision"],
        rev + 2,
        "timed out command must not commit"
    );
    h.sql(&format!(
        "UPDATE account_roles SET is_active=false WHERE user_id={} AND role='administrator'",
        h.actor.id
    ));
    private(
        &post(&h, payload("hold", rev + 2, None), ORIGIN, &h.actor.csrf).await,
        StatusCode::FORBIDDEN,
    );
    private(&h.get(&h.actor, API).await, StatusCode::FORBIDDEN);
    h.sql(&format!(
        "UPDATE account_roles SET is_active=true WHERE user_id={} AND role='administrator'",
        h.actor.id
    ));
    private(
        &post(&h, payload("hold", rev + 2, None), ORIGIN, &h.actor.csrf).await,
        StatusCode::OK,
    );
    assert_eq!(
        optional_snapshot(&h.url, h.actor.id),
        snapshot,
        "context model cannot revive optional collection"
    );
    h.sql("UPDATE rec_runtime_policy SET active_generation=NULL,previous_generation=NULL,rollout_percent=0,hold=true,revision=revision+1 WHERE singleton");
    h.sql(&format!("DELETE FROM rec_generations WHERE id='{id}'; DELETE FROM account_roles WHERE user_id={} AND role='administrator'",h.actor.id));
    h.cleanup();
}
