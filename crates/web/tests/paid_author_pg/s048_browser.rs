//! S048: Chromium against the real Axum router, PostgreSQL and MinIO.
use axum::Router;

pub(super) async fn verify(
    app: Router,
    owner_session: &str,
    other_session: &str,
    csrf: &str,
    book: i64,
    chapter: i64,
) {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3098")
        .await
        .expect("isolated S048 browser listener");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("isolated S048 browser server");
    });
    let owner_session = owner_session.to_owned();
    let other_session = other_session.to_owned();
    let csrf = csrf.to_owned();
    let outcome = tokio::task::spawn_blocking(move || {
        std::process::Command::new("node")
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tools/s048_paid_author_browser.cjs"),
            )
            .env("S048_OWNER_SESSION", owner_session)
            .env("S048_OTHER_SESSION", other_session)
            .env("S048_CSRF", csrf)
            .env("S048_BOOK_ID", book.to_string())
            .env("S048_CHAPTER_ID", chapter.to_string())
            .status()
            .expect("node/playwright in disposable CI")
    })
    .await;
    server.abort();
    let _ = server.await;
    assert!(
        outcome.expect("browser task").success(),
        "real S048 Chromium browser failed"
    );
}
