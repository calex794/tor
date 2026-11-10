use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use serde_json::Value;
use std::{
    num::NonZeroU32,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

pub const ORIGIN: &str = "http://127.0.0.1:3098";
pub const FEEDBACK: &str = "/api/v1/me/recommendation-feedback";

pub struct Actor {
    pub id: i64,
    pub cookie: String,
    pub csrf: String,
}

pub struct Harness {
    pub app: Router,
    pub actor: Actor,
    pub other: Actor,
    pub book: i64,
    pub url: String,
}

impl Harness {
    pub async fn new(enabled: bool) -> Self {
        let setting = if enabled { "1" } else { "0" };
        for name in [
            "TOR_RECOMMENDATIONS_ENABLED",
            "TOR_REC_ATTRIBUTION_ENABLED",
            "TOR_ORGANIC_ATTRIBUTION_ENABLED",
            "TOR_FLOW_VISITS_ENABLED",
            "TOR_READING_RETURNS_ENABLED",
            "TOR_BOOK_READING_RETURNS_ENABLED",
            "TOR_RIL_SINGLE_INSTANCE",
            "TOR_FLOW_PUBLIC_OPT_IN_ENABLED",
        ] {
            std::env::set_var(name, setting);
        }
        let url = std::env::var("TEST_DATABASE_URL").expect("isolated test database");
        assert!(
            url.starts_with("postgresql://")
                && url.contains("@127.0.0.1:5432/")
                && url.ends_with("/tor_ci"),
            "use only the disposable loopback tor_ci database"
        );
        super::assert_current_schema(&url);
        let catalog = PgCatalog::connect(&url).await.unwrap();
        let pool = catalog.pool().clone();
        let accounts = PgAccounts::from_pool(pool.clone());
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let password = hash_password("PreferenceBrowserPassword2026").unwrap();
        let actor = register(&accounts, nonce, "reader", &password).await;
        let other = register(&accounts, nonce, "other", &password).await;
        let book = pg(
            &url,
            &format!(
                "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
                 VALUES('ci:preferences:http:{nonce}','ru',
                   'Preference & <script>fixture</script>','public','basic') RETURNING id"
            ),
        )
        .parse()
        .unwrap();
        let app = tor_web::router_with_postgres_reading_auth(
            catalog,
            PgReader::new(pool),
            accounts,
            false,
            ORIGIN.to_owned(),
        );
        Self {
            app,
            actor,
            other,
            book,
            url,
        }
    }

    pub fn sql(&self, statement: &str) -> String {
        pg(&self.url, statement)
    }

    pub fn preference(&self, actor: &Actor) -> Value {
        serde_json::from_str(&self.sql(&format!(
            "SELECT json_build_object('hidden',hidden,'revision',revision,
               'expires_at',expires_at::text)
             FROM rec_book_preferences WHERE user_id={} AND book_id={}",
            actor.id, self.book
        )))
        .unwrap()
    }

    pub fn command(&self, actor: &Actor, key: &str) -> Value {
        let digest: String = session_token_digest(key)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        serde_json::from_str(&self.sql(&format!(
            "SELECT json_build_object('key',encode(command_key,'hex'),
               'hash',encode(payload_hash,'hex'),'revision',result_revision,
               'kind',command_type,'created_at',created_at::text)
             FROM rec_user_preference_commands
             WHERE user_id={} AND command_key=decode('{digest}','hex')",
            actor.id
        )))
        .unwrap()
    }

    pub fn assert_no_analytics_subject(&self, actor: &Actor) {
        assert_eq!(
            self.sql(&format!(
                "SELECT count(*) FROM rec_subjects WHERE user_id={}",
                actor.id
            )),
            "0",
            "an explicit book preference must not create an analytics subject"
        );
    }

    pub async fn get(&self, actor: &Actor, path: &str) -> Response {
        self.send("GET", path, &actor.cookie, (ORIGIN, &actor.csrf), "", None)
            .await
    }

    pub async fn feedback(
        &self,
        actor: &Actor,
        body: &Value,
        key: &str,
        auth: (&str, &str),
    ) -> Response {
        self.send(
            "POST",
            FEEDBACK,
            &actor.cookie,
            auth,
            &body.to_string(),
            Some(key),
        )
        .await
    }

    pub async fn form(&self, actor: &Actor, path: &str, body: &str) -> Response {
        self.send(
            "POST",
            path,
            &actor.cookie,
            (ORIGIN, &actor.csrf),
            body,
            None,
        )
        .await
    }

    pub async fn send(
        &self,
        method: &str,
        path: &str,
        cookie: &str,
        auth: (&str, &str),
        body: &str,
        key: Option<&str>,
    ) -> Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::COOKIE, cookie)
            .header(header::ORIGIN, auth.0)
            .header("x-csrf-token", auth.1)
            .header(
                header::CONTENT_TYPE,
                if key.is_some() {
                    "application/json"
                } else {
                    "application/x-www-form-urlencoded"
                },
            );
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap()
    }

    pub fn cleanup(&self) {
        self.sql(&format!(
            "DELETE FROM rec_subjects WHERE user_id IN ({},{});
             UPDATE books SET admin_hold=true WHERE id={}",
            self.actor.id, self.other.id, self.book
        ));
    }
}

async fn register(accounts: &PgAccounts, nonce: u128, label: &str, password: &str) -> Actor {
    let session = generate_session_token();
    let csrf = generate_session_token();
    let id = accounts
        .register_with_session(
            &format!("preferences-{label}-{nonce}@tor-ci.example"),
            "Preference reader",
            password,
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    Actor {
        id,
        cookie: format!("tor-dev-session={session}; tor-dev-csrf={csrf}"),
        csrf,
    }
}

pub(super) fn pg(url: &str, statement: &str) -> String {
    let result = Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "--command",
            statement,
        ])
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000",
        )
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

pub fn private(response: &Response, expected: StatusCode) {
    assert_eq!(response.status(), expected);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let robots = response.headers()["x-robots-tag"].to_str().unwrap();
    assert!(robots.contains("noindex") && robots.contains("nofollow"));
}

pub async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

pub async fn json(response: Response) -> Value {
    serde_json::from_str(&text(response).await).unwrap()
}

pub fn choice(book: i64, hidden: bool, revision: i64) -> Value {
    serde_json::json!({
        "book_id": book.to_string(),
        "choice": if hidden { "not_interested" } else { "clear" },
        "expected_revision": revision,
    })
}
