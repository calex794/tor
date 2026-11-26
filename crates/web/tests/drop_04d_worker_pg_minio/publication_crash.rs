//! D-001 / #1178: process death after verified FREE upload, before the PG commit.
use super::{command, pg, wait_ready, Fixture, Server};
use reqwest::{header, redirect::Policy, Client, RequestBuilder, StatusCode};
use std::{
    env,
    io::Write,
    num::NonZeroU32,
    os::unix::process::ExitStatusExt,
    process::{Command, Stdio},
    time::Duration,
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};

const BUCKET: &str = "tor-d001-publication-crash-ci";
const GATE_NAME: &str = "tor-d001-publication-crash-gate";
const V1: &str = "D001 Original V1 — Русский текст 😀";
const V2: &str = "D001 Replacement V2 — Другой текст 中文";

async fn start(fixture: &Fixture, client: &Client) -> (Server, String) {
    let (mut cmd, origin) = command(fixture);
    let mut server = Server(
        cmd.env("TOR_CHAPTER_IMPORT_WORKER_ENABLED", "0")
            .env("TOR_CHAPTER_IMPORT_S3_BUCKET", BUCKET)
            .env("TOR_FREE_S3_BUCKET", BUCKET)
            .spawn()
            .unwrap(),
    );
    wait_ready(&mut server, client, &origin).await;
    (server, origin)
}

fn form(
    client: &Client,
    origin: &str,
    route: &str,
    cookie: &str,
    fields: &[(&str, &str)],
) -> RequestBuilder {
    client
        .post(format!("{origin}{route}"))
        .header(header::ORIGIN, origin)
        .header(header::COOKIE, cookie)
        .form(fields)
}

async fn expect_public(client: &Client, origin: &str, chapter: i64, expected: &str) {
    let response = client
        .get(format!(
            "{origin}/api/v1/chapters/{chapter}/free-grant?format=source"
        ))
        .header(header::ORIGIN, origin)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.unwrap().as_ref(), expected.as_bytes());
}

async fn observed_pid(url: &str, sql: &str, failure: &str) -> i32 {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let pid = pg(url, sql);
            if !pid.is_empty() {
                break pid.parse().expect("exactly one matching PostgreSQL PID");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect(failure)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and loopback MinIO"]
async fn d001_free_author_process_crash_keeps_v1_until_same_command_replays() {
    assert!(
        env::var("TEST_DATABASE_URL").as_deref()
            == Ok("postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_test"),
        "requires disposable CI PostgreSQL"
    );
    let f = Fixture::new_in_bucket("FREE publication crash", BUCKET).await;
    let url = &f.url;
    pg(
        url,
        &format!(
            "UPDATE books SET visibility='public',access_level='basic' WHERE id={}",
            f.book
        ),
    );
    let session = generate_session_token();
    let csrf = generate_session_token();
    PgAccounts::from_pool(f.catalog.pool().clone())
        .insert_session(
            f.actor,
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let cookie = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let client = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(45))
        .build()
        .unwrap();
    let (mut web, origin) = start(&f, &client).await;
    let create = format!("/me/books/{}/chapters", f.book);
    let first = form(
        &client,
        &origin,
        &create,
        &cookie,
        &[
            ("number", "1"),
            ("title", "D001 first"),
            ("body", V1),
            ("csrf_token", &csrf),
            ("command_key", &generate_session_token()),
        ],
    )
    .send()
    .await
    .unwrap();
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    let (chapter, v1, revision): (i64, i64, i64) = serde_json::from_str(&pg(
        url,
        &format!(
            "SELECT json_build_array(c.id,p.version_id,c.edit_revision)
             FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.book_id={} AND NOT EXISTS (
                SELECT 1 FROM chapter_working_drafts d WHERE d.chapter_id=c.id
             )",
            f.book
        ),
    ))
    .expect("published V1 with no draft");
    expect_public(&client, &origin, chapter, V1).await;
    assert_eq!(f.count("free_chapter_storage_objects"), 1);

    // The publication row is read without a lock in preparation. The actual
    // web request can reserve and verify V2 and insert its uncommitted receipt
    // before it hits this row in the final publication UPDATE.
    let mut gate = Server(
        Command::new("psql")
            .args([
                "-X",
                "-A",
                "-t",
                "-q",
                "-v",
                "ON_ERROR_STOP=1",
                "--dbname",
                url,
            ])
            .env("PGAPPNAME", GATE_NAME)
            .env("PGCONNECT_TIMEOUT", "5")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut gate_input = gate.0.stdin.take().unwrap();
    writeln!(
        gate_input,
        "BEGIN; SET LOCAL statement_timeout='5s'; \
         SET LOCAL idle_in_transaction_session_timeout='60s';\n\
         SELECT version_id FROM chapter_publications WHERE chapter_id={chapter} FOR UPDATE;"
    )
    .unwrap();
    let gate_pid = observed_pid(
        url,
        &format!(
            "SELECT pid FROM pg_stat_activity WHERE application_name='{GATE_NAME}'
             AND state='idle in transaction'
             AND query LIKE 'SELECT version_id FROM chapter_publications%'"
        ),
        "the test session must hold the existing publication row",
    )
    .await;

    let route = format!("{create}/{chapter}/edit");
    let key = generate_session_token();
    let rev = revision.to_string();
    let fields = [
        ("number", "1"),
        ("title", "D001 second"),
        ("body", V2),
        ("csrf_token", csrf.as_str()),
        ("command_key", key.as_str()),
        ("expected_revision", rev.as_str()),
    ];
    let edit = tokio::spawn(form(&client, &origin, &route, &cookie, &fields).send());
    let blocked_pid = observed_pid(
        url,
        &format!(
            "SELECT pid FROM pg_stat_activity WHERE state='active' AND wait_event_type='Lock'
             AND query LIKE 'UPDATE chapter_publications SET version_id=%'
             AND {gate_pid}=ANY(pg_blocking_pids(pid))"
        ),
        "the final author UPDATE must be blocked after verified upload",
    )
    .await;
    let find_reserved = format!(
        "SELECT version_id FROM chapter_save_commands
         WHERE user_id={} AND chapter_id={chapter} AND command_key='{key}'",
        f.actor
    );
    let v2: i64 = pg(url, &find_reserved)
        .parse()
        .expect("durable V2 reservation");
    assert_ne!(v1, v2);
    let object = tokio::time::timeout(Duration::from_secs(5), async {
        f.client
            .get_object()
            .bucket(BUCKET)
            .key(format!("private/free-chapters/{chapter}/{v2}.md"))
            .send()
            .await
            .unwrap()
            .body
            .collect()
            .await
            .unwrap()
            .into_bytes()
    })
    .await
    .expect("reserved V2 must already be readable from loopback MinIO");
    assert_eq!(object.as_ref(), V2.as_bytes());
    assert_eq!(
        pg(
            url,
            &format!("SELECT {gate_pid}=ANY(pg_blocking_pids({blocked_pid}))"),
        ),
        "t",
        "publication must still be blocked immediately before SIGKILL",
    );

    web.0.kill().unwrap();
    assert_eq!(web.0.wait().unwrap().signal(), Some(9));
    let interrupted = edit
        .await
        .unwrap()
        .expect_err("killed web cannot acknowledge");
    assert!(
        !interrupted.is_timeout(),
        "must lose the socket, not the HTTP timeout"
    );
    writeln!(gate_input, "ROLLBACK;").unwrap();
    drop(gate_input);
    assert!(gate.0.wait().unwrap().success());

    // The server can finish a blocked statement after socket loss, but cannot
    // receive the separate COMMIT. Wait for its session/rollback before asserting.
    tokio::time::timeout(Duration::from_secs(15), async {
        while pg(
            url,
            &format!("SELECT count(*) FROM pg_stat_activity WHERE pid={blocked_pid}"),
        ) != "0"
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("dead web backend must resolve after the gate is released");

    let state_query = format!(
        "SELECT json_build_array(p.version_id,c.edit_revision,
          (SELECT count(*) FROM free_chapter_storage_objects
           WHERE chapter_id=c.id AND version_id={v2}),
          (SELECT count(*) FROM chapter_versions WHERE chapter_id=c.id))
         FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
         WHERE c.id={chapter}"
    );
    let state = || -> (i64, i64, i64, i64) {
        serde_json::from_str(&pg(url, &state_query)).unwrap()
    };
    assert_eq!(state(), (v1, revision, 0, 2));
    assert_eq!(pg(url, &find_reserved), v2.to_string());
    assert_eq!(f.count("free_chapter_storage_objects"), 1);

    let (_restarted, new_origin) = start(&f, &client).await;
    expect_public(&client, &new_origin, chapter, V1).await;
    assert_eq!(
        state(),
        (v1, revision, 0, 2),
        "restart alone cannot publish V2"
    );
    for _ in 0..2 {
        let reply = form(&client, &new_origin, &route, &cookie, &fields)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::SEE_OTHER);
        assert_eq!(pg(url, &find_reserved), v2.to_string());
        assert_eq!(state(), (v2, revision + 1, 1, 2));
        expect_public(&client, &new_origin, chapter, V2).await;
    }
    println!("D001_FREE_AUTHOR_CRASH_BEFORE_COMMIT_REPLAY_OK");
}
