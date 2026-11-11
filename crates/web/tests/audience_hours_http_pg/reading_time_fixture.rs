use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{
    io::{BufRead, BufReader, Write},
    num::NonZeroU32,
    process::{Child, ChildStdin, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

pub const FALL: &str = "2026-10-25T04:37:00Z";
const ORIGIN: &str = "http://127.0.0.1:3120";
fn check_url(raw: &str, role: &str) {
    let url = reqwest::Url::parse(raw).unwrap();
    let port: u16 = std::env::var("FLOW_TEST_PG_PORT")
        .unwrap_or_else(|_| "5432".into())
        .parse()
        .unwrap();
    assert!(port > 0);
    assert_eq!(url.scheme(), "postgresql");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.port(), Some(port));
    assert_eq!(url.username(), role);
    assert_eq!(url.path(), "/tor_ci");
    assert!(url.query().is_none() && url.fragment().is_none());
}
fn command(url: &str) -> Command {
    let mut cmd = Command::new("psql");
    cmd.args(["-XAtq", "-v", "ON_ERROR_STOP=1", "--dbname", url])
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000",
        );
    cmd
}
fn sql(url: &str, statement: &str) -> String {
    let output = command(url).args(["-c", statement]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
pub struct Fixture {
    pub app: Router,
    pub accounts: PgAccounts,
    pub catalog: PgCatalog,
    pub cookie: String,
    pub token: String,
    pub actor: i64,
    pub schema: String,
    url: String,
    owner: String,
}
impl Fixture {
    pub async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let owner = std::env::var("MIGRATOR_URL").unwrap();
        // Validate both authorities before the first connection or write.
        check_url(&url, "tor_app");
        check_url(&owner, "tor_migrator");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let schema = format!("flow_reading_time_http_{stamp}");
        let base = PgCatalog::connect(&url).await.unwrap();
        let accounts = PgAccounts::from_pool(base.pool());
        let token = generate_session_token();
        let actor = accounts
            .register_with_session(
                &format!("reading-time-{stamp}@example.invalid"),
                "Clock-only fixture",
                "$argon2id$test",
                &session_token_digest(&token),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        base.close_for_test().await;
        let mut setup =
            format!("CREATE SCHEMA {schema}; GRANT USAGE ON SCHEMA {schema} TO tor_app;");
        for table in [
            "reader_chapter_starts",
            "reader_chapter_completions",
            "outbox_events",
        ] {
            setup.push_str(&format!(
                "CREATE TABLE {schema}.{table} (LIKE public.{table} INCLUDING ALL);"
            ));
        }
        setup.push_str(&format!(
            "REVOKE ALL ON ALL TABLES IN SCHEMA {schema} FROM tor_app;
             GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO tor_app;"
        ));
        sql(&owner, &setup);
        let mut scoped = reqwest::Url::parse(&url).unwrap();
        // Explicit pg_catalog position makes only our test clock override visible.
        scoped.query_pairs_mut().append_pair(
            "options",
            &format!("-c search_path={schema},pg_catalog,public"),
        );
        let catalog = PgCatalog::connect(scoped.as_str()).await.unwrap();
        let accounts = PgAccounts::from_pool(catalog.pool());
        let app = tor_web::router_with_postgres_reading_auth(
            catalog.clone(),
            PgReader::new(catalog.pool()),
            accounts.clone(),
            false,
            ORIGIN.into(),
        );
        let f = Self {
            app,
            accounts,
            catalog,
            cookie: format!("tor-dev-session={token}"),
            token,
            actor,
            schema,
            url,
            owner,
        };
        f.clock(FALL);
        f
    }
    pub fn scope(&self, statement: &str) -> String {
        sql(
            &self.owner,
            &format!(
                "SET search_path={},pg_catalog,public; {statement}",
                self.schema
            ),
        )
    }
    pub fn clock(&self, value: &str) {
        assert!(["2026-10-25T04:37:00Z", "2026-03-30T00:37:00Z"].contains(&value));
        self.scope(&format!(
            "CREATE OR REPLACE FUNCTION {}.transaction_timestamp() RETURNS timestamptz
             LANGUAGE SQL STABLE AS 'SELECT ''{value}''::timestamptz';
             GRANT EXECUTE ON FUNCTION {}.transaction_timestamp() TO tor_app;",
            self.schema, self.schema
        ));
    }
    pub fn role(&self, enabled: bool) {
        sql(
            &self.url,
            &format!(
            "INSERT INTO account_roles(user_id,role,is_active) VALUES({},'administrator',{enabled})
             ON CONFLICT(user_id,role) DO UPDATE SET is_active=EXCLUDED.is_active", self.actor
        ),
        );
    }
    pub fn seed(&self) {
        for (chapter, started, completed) in [
            (7000, "2026-03-01T00:01:00Z", "2026-03-01T00:05:00Z"),
            (7001, "2026-10-24T23:30:00Z", "2026-10-25T00:30:00Z"),
            (7002, "2026-10-25T00:45:00Z", "2026-10-25T01:30:00Z"),
            (7003, "2026-10-24T03:30:00Z", "2026-10-24T03:45:00Z"),
            (7004, "2026-10-25T04:01:00Z", "2026-10-25T04:02:00Z"),
        ] {
            for (table, column, at) in [
                ("reader_chapter_starts", "started_at", started),
                ("reader_chapter_completions", "completed_at", completed),
            ] {
                self.scope(&format!(
                    "INSERT INTO {table}(user_id,book_id,chapter_id,first_version_id,source_event_id,{column})
                     VALUES({},6000,{chapter},8000,gen_random_uuid(),'{at}')", self.actor
                ));
                self.proof(chapter, table == "reader_chapter_completions");
            }
        }
    }
    pub fn proof(&self, chapter: i64, completed: bool) {
        let (table, event, authority) = if completed {
            (
                "reader_chapter_completions",
                "ChapterCompletedV1",
                "C-verified-free-position",
            )
        } else {
            (
                "reader_chapter_starts",
                "ChapterStartedV1",
                "C-verified-free-start",
            )
        };
        self.scope(&format!(
            "INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
             SELECT source_event_id,'{event}',1,'chapter',chapter_id,
             'clock:'||source_event_id::text,
             jsonb_build_object('user_id',user_id,'book_id',book_id,'chapter_id',chapter_id,'version_id',first_version_id,'authority','{authority}'),
             ARRAY[]::text[] FROM {table} WHERE chapter_id={chapter}"
        ));
    }
    pub fn fingerprint(&self) -> String {
        self.scope(
            "SELECT md5(jsonb_build_array(
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY chapter_id) FROM reader_chapter_starts t),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY chapter_id) FROM reader_chapter_completions t),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM outbox_events t))::text)",
        )
    }
    pub async fn get(&self, path: &str, cookie: &str, expected: StatusCode) -> String {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
        assert!(response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("script-src 'none'"));
        String::from_utf8(
            to_bytes(response.into_body(), 256 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }
    pub fn lock(&self) -> Lock {
        let mut child = command(&self.owner)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        writeln!(
            input,
            "BEGIN; LOCK TABLE {}.outbox_events IN ACCESS EXCLUSIVE MODE; SELECT 'locked';",
            self.schema
        )
        .unwrap();
        input.flush().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "locked");
        Lock { child, input }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        sql(
            &self.owner,
            &format!("DROP SCHEMA {} CASCADE;", self.schema),
        );
        sql(
            &self.url,
            &format!(
                "DELETE FROM account_roles WHERE user_id={};DELETE FROM users WHERE id={};",
                self.actor, self.actor
            ),
        );
    }
}
pub struct Lock {
    child: Child,
    input: ChildStdin,
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "ROLLBACK;\n\\q");
        let _ = self.input.flush();
        let _ = self.child.wait();
    }
}
pub fn section<'a>(html: &'a str, marker: &str) -> &'a str {
    html.split(marker)
        .nth(1)
        .expect(marker)
        .split("</section>")
        .next()
        .unwrap()
}
pub fn metric<'a>(html: &'a str, key: &str) -> &'a str {
    html.split(&format!("data-metric=\"{key}\""))
        .nth(1)
        .unwrap()
        .split('>')
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
}
