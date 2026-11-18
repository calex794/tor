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
            &format!(
                "SELECT count(body_markdown) FROM chapter_versions WHERE chapter_id={chapter}"
            ),
        ),
        "0",
        "paid browser Save must never persist Markdown in PostgreSQL"
    );
    let route = format!("/me/books/{book}/chapters/{chapter}/edit");
    let (status, owner_source) = browse(app, &route, owner_session, csrf).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "S3-only paid editor must open for owner"
    );
    assert!(owner_source.contains("Paid chapter V2"));
    let (status, _) = browse(app, &route, other_session, csrf).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Read the immutable illustration binding, verifying the paid text is S3-only.
pub(super) fn illustrated_identity(
    url: &str,
    book: i64,
) -> (i64, tor_contracts::paid_delivery::ObjectId) {
    let published: String = pg(
        url,
        &format!(
            "SELECT v.id || '|' || coalesce(v.body_markdown,'<NULL>') || '|' || a.storage_object_id::text
             FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.id=p.version_id
             JOIN chapter_version_assets a ON a.chapter_id=c.id AND a.version_id=v.id
             WHERE c.book_id={book} AND c.chapter_no=2"
        ),
    );
    let parts = published.split('|').collect::<Vec<_>>();
    assert_eq!(parts.len(), 3);
    assert_eq!(
        parts[1], "<NULL>",
        "illustrated paid edition must also be S3-only without a PostgreSQL body"
    );
    (
        parts[0].parse().expect("valid paid version"),
        tor_contracts::paid_delivery::ObjectId::parse(parts[2]).expect("valid asset identity"),
    )
}
