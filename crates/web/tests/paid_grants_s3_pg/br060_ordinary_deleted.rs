//! BR-060: a finalized ordinary deletion keeps an immutable paid V1 readable.
//! The caller passes this fresh grant to the real MinIO/WebCrypto consumer.
use super::*;

/// Returns a grant issued *after* removal; neither a stale signed URL nor
/// a database-only entitlement is sufficient for downstream S3 acceptance.
pub(super) async fn verify(
    app: &Router,
    url: &str,
    book: i64,
    purchased: (i64, i64, i64),
    sessions: (&str, &str),
    csrf: &str,
) -> Value {
    let (chapter, purchase_id, v1) = purchased;
    let (buyer_cookie, stranger_cookie) = sessions;
    pg(
        url,
        &format!(
            "UPDATE books SET lifecycle_status='ordinary_deleted', \
             ordinary_deleted_at=now(), visibility='draft' WHERE id={book}"
        ),
    );
    assert_eq!(
        private_get(app, &format!("/books/{book}"), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let archived_reader = paid_page(app, chapter, purchase_id, buyer_cookie).await;
    assert_eq!(archived_reader.status(), StatusCode::OK);
    let archived_html = html_body(archived_reader).await;
    assert!(archived_html.contains("data-paid-chapter-reader"));
    assert!(!archived_html.contains("data-toc-state=\"free\""));
    assert!(!archived_html.contains("data-toc-state=\"locked\""));
    assert_eq!(
        post_grant(
            app,
            chapter,
            Some(stranger_cookie),
            Some(csrf),
            ORIGIN,
            "{}",
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let authorized = post_grant(app, chapter, Some(buyer_cookie), Some(csrf), ORIGIN, "{}").await;
    assert_eq!(authorized.status(), StatusCode::OK);
    let grant = decode(authorized).await;
    assert_eq!(grant["version_id"], v1.to_string());
    assert!(!grant.to_string().contains(MARKDOWN));
    grant
}
