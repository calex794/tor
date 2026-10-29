//! Real private-S3 regression assertions for exact direct paid V1 evidence.
use super::*;

pub(super) fn assert_not_issued(url: &str, purchase_id: i64) {
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT COUNT(*) FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        0,
        "checkout itself cannot record first protected issuance"
    );
}

pub(super) fn assert_first_issued(
    url: &str,
    purchase_id: i64,
    chapter: i64,
    v1: i64,
    purchase_two_id: i64,
) {
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT chapter_id || ':' || version_id || ':' || object_kind
                 FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_id}"
            ),
        ),
        format!("{chapter}:{v1}:chapter"),
        "real paid MinIO response must mark the exact direct purchased V1"
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT COUNT(*) FROM chapter_purchase_first_exposures WHERE purchase_id={purchase_two_id}"
            ),
        ),
        0,
        "opening one purchased chapter cannot mark a different purchase"
    );
}

/// S035 checkout creates independent direct purchase IDs. A grant for one
/// chapter must not mark its sibling opened; only a later authenticated ACK
/// may append the first-render fact. This simulates the client ACK; a distinct
/// Chromium test proves the browser sends it only after verifying text.
pub(super) async fn assert_batch_first_render(
    app: &Router,
    pool: &sqlx::PgPool,
    ids: (i64, i64, i64, i64, i64),
    auth: (&str, &str),
) {
    use tor_commerce::{
        repositories::refund_use_evidence::PgRefundUseEvidence,
        services::refunds::UseEvidence,
    };
    let (buyer, chapter, version, first, second) = ids;
    let (session, csrf) = auth;
    let audit = PgRefundUseEvidence::new(pool.clone());
    let actor = PositiveId::new(buyer).unwrap();
    assert_eq!(
        audit.direct(actor, PositiveId::new(first).unwrap())
            .await.unwrap().evidence,
        UseEvidence::ChapterTextGrantUnconfirmed,
    );
    assert_eq!(
        audit.direct(actor, PositiveId::new(second).unwrap())
            .await.unwrap().evidence,
        UseEvidence::Unknown,
    );
    let endpoint = format!("/api/v1/reader/chapters/{chapter}/open-confirmations");
    let make = |purchase: i64| {
        Request::builder()
            .method("POST")
            .uri(&endpoint)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE,
                format!("tor-dev-session={session}; tor-dev-csrf={csrf}"))
            .header("x-csrf-token", csrf)
            .body(Body::from(json!({
                "purchase_id": purchase.to_string(),
                "version_id": version.to_string(),
            }).to_string()))
            .unwrap()
    };
    // An owned, but DIFFERENT batch purchase cannot authorize chapter A.
    assert_eq!(app.clone().oneshot(make(second)).await.unwrap().status(),
        StatusCode::FORBIDDEN);
    assert_eq!(app.clone().oneshot(make(first)).await.unwrap().status(),
        StatusCode::OK);
    let timestamp = pg(&env::var("TEST_DATABASE_URL").unwrap(), &format!(
        "SELECT confirmed_at::TEXT FROM chapter_purchase_render_confirmations WHERE purchase_id={first}"
    ));
    assert_eq!(app.clone().oneshot(make(first)).await.unwrap().status(),
        StatusCode::OK);
    assert_eq!(pg(&env::var("TEST_DATABASE_URL").unwrap(), &format!(
        "SELECT confirmed_at::TEXT FROM chapter_purchase_render_confirmations WHERE purchase_id={first}"
    )), timestamp, "replayed ACK never rewrites the first opening");
    assert_eq!(
        audit.direct(actor, PositiveId::new(first).unwrap())
            .await.unwrap().evidence,
        UseEvidence::BrowserRenderReported,
    );
    assert_eq!(
        audit.direct(actor, PositiveId::new(second).unwrap())
            .await.unwrap().evidence,
        UseEvidence::Unknown,
        "opening one of two batch purchases must not consume its sibling",
    );
}
