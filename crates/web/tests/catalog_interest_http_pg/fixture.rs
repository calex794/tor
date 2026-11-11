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

pub const ORIGIN: &str = "http://127.0.0.1:3118";
pub fn sql(url: &str, statement: &str) -> String {
    let parsed = reqwest::Url::parse(url).unwrap();
    assert_eq!(parsed.scheme(), "postgresql");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    let port = std::env::var("FLOW_TEST_PG_PORT").unwrap_or_else(|_| "5432".into()).parse::<u16>().unwrap();
    assert!(port > 0);
    assert_eq!(parsed.port(), Some(port));
    assert_eq!(parsed.path(), "/tor_ci");
    assert!(parsed.query().is_none() && parsed.fragment().is_none());
    let result = Command::new("psql")
        .args(["-XAtq", "-v", "ON_ERROR_STOP=1", "--dbname", url, "-c", statement])
        .env("PGOPTIONS", "-c statement_timeout=5000 -c lock_timeout=2000")
        .output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    String::from_utf8(result.stdout).unwrap().trim().into()
}
pub struct Fixture {
    pub app: Router,
    pub catalog: PgCatalog,
    pub accounts: PgAccounts,
    pub url: String,
    pub owner: String,
    pub schema: String,
    pub tag: String,
    pub actor: i64,
    pub token: String,
    pub cookie: String,
    pub books: Vec<i64>,
    pub anchor: i64,
}
impl Fixture {
    pub async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let owner = std::env::var("MIGRATOR_URL").unwrap();
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let tag = format!("interest{stamp}");
        let schema = format!("flow_catalog_ui_{stamp}");
        let base = PgCatalog::connect(&url).await.unwrap();
        let accounts = PgAccounts::from_pool(base.pool());
        let token = generate_session_token();
        let actor = accounts.register_with_session(
            &format!("{tag}@example.invalid"), "Catalogue report fixture", "$argon2id$test",
            &session_token_digest(&token), NonZeroU32::new(3600).unwrap(),
        ).await.unwrap();
        base.close_for_test().await;
        let books: Vec<i64> = sql(&url, &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level,tags,genres)
             SELECT 'ci:{tag}:'||n,'ru','Catalogue fixture '||n,'public','basic',
             ARRAY['{tag}'],ARRAY['catalogue-default'] FROM generate_series(1,53)n RETURNING id"
        )).lines().map(|id| id.parse().unwrap()).collect();
        assert_eq!(books.len(), 53);
        let mut setup = format!("CREATE SCHEMA {schema}; GRANT USAGE ON SCHEMA {schema} TO tor_app;");
        for table in ["reader_chapter_starts", "reader_chapter_completions", "outbox_events",
            "reader_library_items", "reader_book_follows", "flow_observation_hourly",
            "flow_observation_epochs", "flow_observation_health_hourly"] {
            setup.push_str(&format!(
                "CREATE TABLE {schema}.{table} (LIKE public.{table} INCLUDING ALL);"
            ));
        }
        setup.push_str(&format!("REVOKE ALL ON ALL TABLES IN SCHEMA {schema} FROM tor_app;
            GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO tor_app;"));
        sql(&owner, &setup);
        let mut scoped = reqwest::Url::parse(&url).unwrap();
        scoped.query_pairs_mut().append_pair("options", &format!("-c search_path={schema},public"));
        let catalog = PgCatalog::connect(scoped.as_str()).await.unwrap();
        let accounts = PgAccounts::from_pool(catalog.pool());
        let app = tor_web::router_with_postgres_reading_auth(
            catalog.clone(), PgReader::new(catalog.pool()), accounts.clone(), false, ORIGIN.into(),
        );
        let (runtime, worker) = tor_web::flow_exposure::Runtime::new(accounts.clone(), ORIGIN.into(), false, None);
        drop(worker);
        let app = runtime.attach_routes(app);
        let anchor = sql(&url, "SELECT floor((extract(epoch FROM now())-900)/3600)::bigint*3600").parse().unwrap();
        let f = Self { app, catalog, accounts, url, owner, schema, tag, actor,
            cookie: format!("tor-dev-session={token}"), token, books, anchor };
        f.seed();
        f
    }
    pub fn scope(&self, statement: &str) -> String {
        sql(&self.owner, &format!("SET search_path={},public; {statement}", self.schema))
    }
    pub fn seed(&self) {
        for (index, count) in [(50, 10), (51, 10), (52, 9)] {
            let book = self.books[index];
            for user in 1..=count {
                self.event(book, user, book * 100 + user, self.anchor - 7200, true, true);
            }
        }
        for user in 101..=110 {
            self.event(self.books[51], user, self.books[51] * 100 + user, self.anchor - 8 * 86400, true, true);
        }
        // Its first account/book completion is old, but the next chapter is in this window.
        self.event(self.books[0], 500, 50001, self.anchor - 200 * 86400, true, true);
        self.event(self.books[0], 500, 50002, self.anchor - 7200, true, true);
        self.event(self.books[0], 500, 50002, self.anchor - 10800, false, true);
        self.event(self.books[2], 600, 60001, self.anchor - 7200, true, false);
        sql(&self.url, &format!("UPDATE books SET genres=ARRAY['shared','a','a'],title='Rank <img src=x onerror=alert(1)>' WHERE id={};
            UPDATE books SET genres=ARRAY['shared','b'] WHERE id={};
            UPDATE books SET genres=ARRAY[]::text[] WHERE id={};", self.books[50], self.books[51], self.books[52]));
        self.scope(&format!(
            "INSERT INTO flow_observation_hourly(bucket,book_id,surface,opportunities,rendered,views)
             SELECT to_timestamp({}),id,'catalog',25,CASE WHEN id={} THEN 19 ELSE 20 END,5
             FROM books WHERE tags @> ARRAY['{}']", self.anchor - 7200, self.books[3], self.tag
        ));
    }
    pub fn event(&self, book: i64, user: i64, chapter: i64, at: i64, completed: bool, verified: bool) {
        let (table, column, kind, authority) = if completed {
            ("reader_chapter_completions", "completed_at", "ChapterCompletedV1", "C-verified-free-position")
        } else { ("reader_chapter_starts", "started_at", "ChapterStartedV1", "C-verified-free-start") };
        self.scope(&format!(
            "WITH fact AS (
             INSERT INTO {table}(user_id,book_id,chapter_id,first_version_id,{column},source_event_id)
             VALUES({user},{book},{chapter},{chapter},to_timestamp({at}),gen_random_uuid()) RETURNING *)
             INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
             SELECT source_event_id,'{kind}',1,'chapter',chapter_id,source_event_id::text,
             jsonb_build_object('user_id',user_id,'book_id',book_id,'chapter_id',chapter_id,
                 'version_id',first_version_id,'authority','{authority}'),ARRAY['fixture-only']
             FROM fact WHERE {verified}"
        ));
    }
    pub fn promote(&self) {
        sql(&self.url, &format!("INSERT INTO account_roles(user_id,role) VALUES({},'administrator')", self.actor));
    }
    pub fn certify(&self) -> i64 {
        let from = (self.anchor - 2 * 86400) * 1000;
        let through = (self.anchor + 902) * 1000;
        self.scope(&format!(
            "INSERT INTO flow_observation_epochs(started_ms,confirmed_ms,lease_until_ms,retired,ended_ms)
             VALUES({from},{through},{through},true,{through}) RETURNING epoch"
        )).parse().unwrap()
    }
    pub fn fingerprint(&self) -> String {
        self.scope("SELECT md5(jsonb_build_array(
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY user_id,book_id,chapter_id) FROM reader_chapter_starts t),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY user_id,book_id,chapter_id) FROM reader_chapter_completions t),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM outbox_events t),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY book_id) FROM flow_observation_hourly t),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY epoch) FROM flow_observation_epochs t),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY epoch,bucket_ms) FROM flow_observation_health_hourly t)
        )::text)")
    }
    pub fn rank_url(&self) -> String {
        format!("/admin/audience/reading?days=7&tag={}&view=top20", self.tag)
    }
    pub fn exposure_url(&self) -> String {
        format!("/admin/audience/exposure?days=1&window=closed&group=tag&label={}", self.tag)
    }
    pub async fn get(&self, path: &str, cookie: &str, status: StatusCode) -> String {
        let response = self.app.clone().oneshot(
            Request::get(path).header(header::COOKIE, cookie).body(Body::empty()).unwrap()
        ).await.unwrap();
        assert_eq!(response.status(), status, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        String::from_utf8(to_bytes(response.into_body(), 2 * 1024 * 1024).await.unwrap().to_vec()).unwrap()
    }
    pub async fn finish(self) { self.catalog.close_for_test().await; }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        sql(&self.owner, &format!("DROP SCHEMA {} CASCADE", self.schema));
        sql(&self.url, &format!(
            "DELETE FROM books WHERE source_identity LIKE 'ci:{}:%';
             DELETE FROM account_roles WHERE user_id={}; DELETE FROM users WHERE id={}",
            self.tag, self.actor, self.actor
        ));
    }
}
pub struct HeldLock(Child, Option<ChildStdin>);
impl HeldLock {
    pub fn acquire(f: &Fixture, table: &str) -> Self {
        assert!(["reader_chapter_starts", "outbox_events"].contains(&table));
        let mut child = Command::new("psql")
            .args(["-XAtq", "-v", "ON_ERROR_STOP=1", "--dbname", &f.owner])
            .env("PGOPTIONS", "-c idle_in_transaction_session_timeout=5000")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        writeln!(input, "BEGIN; LOCK TABLE {}.{table} IN ACCESS EXCLUSIVE MODE; SELECT 'HELD';", f.schema).unwrap();
        input.flush().unwrap();
        let mut line = String::new(); output.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "HELD");
        Self(child, Some(input))
    }
}
impl Drop for HeldLock {
    fn drop(&mut self) {
        if let Some(mut input) = self.1.take() {
            let _ = writeln!(input, "ROLLBACK;\n\\q");
            let _ = input.flush();
        }
        let _ = self.0.wait();
    }
}
pub fn row(html: &str, testid: &str, book: i64) -> String {
    let marker = format!("data-testid=\"{testid}\"");
    html.split(&marker).skip(1).find(|part|
        part.split("</article>").next().unwrap().contains(&format!("data-book-id=\"{book}\""))
    ).expect("book row").split("</article>").next().unwrap().into()
}
pub fn metric(html: &str, name: &str) -> String {
    html.split(&format!("data-metric=\"{name}\">")).nth(1).unwrap()
        .split('<').next().unwrap().into()
}
pub fn ids(html: &str, testid: &str) -> Vec<i64> {
    html.split(&format!("data-testid=\"{testid}\"")).skip(1).map(|part|
        part.split("data-book-id=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap()
    ).collect()
}
