//! S3-only paid browser acceptance: plaintext is owner-only and absent from PG.
use super::*;

pub(super) async fn assert_owner_editor(
    app: &Router,
    url: &str,
    book: i64,
    chapter: i64,
    owner_session: &str,
    other_session: &str,
    csrf: &str,
) {
    assert_eq!(
        pg(
            url,
            &format!("SELECT count(body_markdown) FROM chapter_versions WHERE chapter_id={chapter}"),
        ),
        "0",
        "paid browser Save must never persist Markdown in PostgreSQL"
    );
    let route = format!("/me/books/{book}/chapters/{chapter}/edit");
    let (status, owner_source) = browse(app, &route, owner_session, csrf).await;
    assert_eq!(status, StatusCode::OK, "S3-only paid editor must open for owner");
    assert!(owner_source.contains("Paid chapter V2"));
    let (status, _) = browse(app, &route, other_session, csrf).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
