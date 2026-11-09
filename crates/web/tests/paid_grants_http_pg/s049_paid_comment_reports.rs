//! S049: real isolated PG/HTTP acceptance for report on a pinned purchased V1.
use super::*;

pub(super) async fn check(
    url: &str,
    browser: &Router,
    actors: (i64, i64, &str, &str),
    edition: (i64, i64, i64, i64),
    tokens: (&str, &str),
) {
    let (buyer, author, buyer_cookie, author_cookie) = actors;
    let (book, chapter, version, purchase) = edition;
    let (csrf, author_csrf) = tokens;
    let comment: i64 = scalar(
        url,
        &format!(
            "INSERT INTO discussion_comments
             (author_user_id,chapter_id,version_id,body,visibility,command_key,command_hash)
             VALUES({author},{chapter},{version},'S049 paid private comment','visible',
                    '{}',decode(repeat('08',32),'hex')) RETURNING id",
            "P".repeat(43),
        ),
    );
    let route = format!("{ENDPOINT}{chapter}/paid-comments/report");
    let key = "Q".repeat(43);
    let input = json!({
        "purchase_id":purchase.to_string(), "version_id":version.to_string(),
        "comment_id":comment.to_string(), "reason":"abuse",
        "details":"Paid comment complaint <script>x()</script>", "command_key":key
    });
    let raw = input.to_string();
    assert_eq!(
        send(browser, &route, None, Some(csrf), Some(ORIGIN), &raw)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            browser,
            &route,
            Some(author_cookie),
            Some(author_csrf),
            Some(ORIGIN),
            &raw
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "a stranger cannot submit a report using another buyer's purchase"
    );
    assert_eq!(
        send(
            browser,
            &route,
            Some(buyer_cookie),
            Some(csrf),
            Some("https://evil.invalid"),
            &raw
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            browser,
            &route,
            Some(buyer_cookie),
            None,
            Some(ORIGIN),
            &raw
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            browser,
            &route,
            Some(buyer_cookie),
            Some(csrf),
            Some(ORIGIN),
            &json!({"purchase_id":purchase.to_string(),
                "version_id":version.to_string(), "comment_id":"0",
                "reason":"abuse","details":"bad","command_key":key})
            .to_string()
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let wrong_edition = json!({
        "purchase_id":purchase.to_string(), "version_id":(version+1).to_string(),
        "comment_id":comment.to_string(), "reason":"abuse",
        "details":"test", "command_key":"Z".repeat(43),
    });
    assert_eq!(
        send(
            browser,
            &route,
            Some(buyer_cookie),
            Some(csrf),
            Some(ORIGIN),
            &wrong_edition.to_string()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let success = send(
        browser,
        &route,
        Some(buyer_cookie),
        Some(csrf),
        Some(ORIGIN),
        &raw,
    )
    .await;
    assert_eq!(success.status(), StatusCode::OK);
    assert_eq!(
        success.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let response = json_body(success).await;
    let id: i64 = response["id"].as_i64().unwrap();
    assert!(id > 0);
    assert!(!response.to_string().contains("S049 paid private comment"));
    assert!(!response.to_string().contains(BODY));
    assert!(!response.to_string().contains("ciphertext_url"));
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM social_paid_comment_reports
             WHERE id={id} AND chapter_id={chapter} AND version_id={version}
             AND comment_id={comment} AND reporter_user_id={buyer} AND state='pending'"
            )
        ),
        1
    );
    assert_eq!(
        send(
            browser,
            &route,
            Some(buyer_cookie),
            Some(csrf),
            Some(ORIGIN),
            &raw
        )
        .await
        .status(),
        StatusCode::OK,
        "identical command key replays exactly once"
    );
    assert_eq!(
        scalar(
            url,
            &format!("SELECT count(*) FROM social_paid_comment_reports WHERE comment_id={comment}")
        ),
        1
    );
    let changed_details = raw.replace("Paid comment complaint", "Changed complaint");
    assert_eq!(
        send(
            browser,
            &route,
            Some(buyer_cookie),
            Some(csrf),
            Some(ORIGIN),
            &changed_details
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let second_key = raw.replace(&key, &"R".repeat(43));
    assert_eq!(
        send(
            browser,
            &route,
            Some(buyer_cookie),
            Some(csrf),
            Some(ORIGIN),
            &second_key
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "one pending complaint per buyer and exact comment"
    );
    let other_comment: i64 = scalar(
        url,
        &format!(
            "INSERT INTO discussion_comments
             (author_user_id,book_id,body,visibility,command_key,command_hash)
             VALUES({author},{book},'Different public Book comment','visible',
                    '{}',decode(repeat('09',32),'hex')) RETURNING id",
            "S".repeat(43),
        ),
    );
    let forged_comment = raw
        .replace(
            &format!("\"comment_id\":\"{comment}\""),
            &format!("\"comment_id\":\"{other_comment}\""),
        )
        .replace(&key, &"T".repeat(43));
    assert_eq!(
        send(
            browser,
            &route,
            Some(buyer_cookie),
            Some(csrf),
            Some(ORIGIN),
            &forged_comment
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "Book comments are not paid chapter V1 complaint targets"
    );
    pg(
        url,
        &format!("UPDATE discussion_comments SET visibility='hidden' WHERE id={comment}"),
    );
    let stale = raw.replace(&key, &"U".repeat(43));
    assert_eq!(
        send(
            browser,
            &route,
            Some(buyer_cookie),
            Some(csrf),
            Some(ORIGIN),
            &stale
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "hidden comments cannot accept a new complaint"
    );
}
