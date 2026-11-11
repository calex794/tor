//! S049: real PG + HTTP acceptance for independent paid comment sanction appeals.
//! No paid chapter purchase, sealed body, or object grant is needed to file.
use super::*;

async fn post_form(
    app: &Router,
    path: &str,
    token: &str,
    csrf: &str,
    origin: &str,
    form: &str,
) -> Response {
    let req = Request::builder()
        .uri(path)
        .method("POST")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ORIGIN, origin)
        .header(
            header::COOKIE,
            format!("tor-dev-session={token}; tor-dev-csrf={csrf}"),
        )
        .body(Body::from(form.to_owned()))
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15), app.clone().oneshot(req))
        .await
        .expect("S049 paid appeal HTTP request timed out")
        .unwrap()
}

pub(super) async fn check(
    url: &str,
    accounts: &PgAccounts,
    app: &Router,
    author_cookie: &str,
    sanctioned: (i64, i64, i64),
    original_admin_cookie: &str,
) {
    let (comment, hide_report, original_admin) = sanctioned;
    eprintln!("S049_PAID_APPEAL beginning");
    let path = "/me/paid-comment-appeals";
    let admin_path = "/admin/paid-comment-appeals";
    let hide_event = scalar(
        url,
        &format!(
            "SELECT id FROM social_paid_comment_review_events
             WHERE report_id={hide_report} AND moderation_action='hide_paid_comment'"
        ),
    );
    let author: i64 = scalar(
        url,
        &format!("SELECT author_user_id FROM discussion_comments WHERE id={comment}"),
    );
    let (independent, reviewer_cookie) =
        account(accounts, "s049-paid-independent-review@tor-ci.example").await;
    let (_, unrelated_cookie) = account(accounts, "s049-paid-appeal-outsider@tor-ci.example").await;
    pg(
        url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({independent},'administrator',TRUE)
             ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE"
        ),
    );
    assert_eq!(
        read_page(app, path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let unrelated = read_page(app, path, Some(&unrelated_cookie)).await;
    let unrelated_text = String::from_utf8(
        to_bytes(unrelated.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!unrelated_text.contains(&format!("решение #{hide_event}")));
    assert_eq!(
        read_page(app, admin_path, Some(author_cookie))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let author_page = read_page(app, path, Some(author_cookie)).await;
    assert_eq!(author_page.status(), StatusCode::OK);
    assert_eq!(author_page.headers()[header::CACHE_CONTROL], "no-store");
    let author_html = String::from_utf8(
        to_bytes(author_page.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(author_html.contains(&format!("решение #{hide_event}")));
    assert!(!author_html.contains(BODY));
    assert!(!author_html.contains("ciphertext_url"));

    eprintln!("S049_PAID_APPEAL reached author browser and role checks");
    let token = generate_session_token();
    let reason = "Request+independent+review+of+my+hidden+comment";
    let key = "A".repeat(43);
    let appeal_url = format!("{path}/{hide_event}");
    let form = format!("csrf_token={token}&command_key={key}&reason={reason}");
    assert_eq!(
        post_form(app, &appeal_url, &unrelated_cookie, &token, ORIGIN, &form)
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "the hide event does not authorize another user"
    );
    assert_eq!(
        post_form(
            app,
            &appeal_url,
            author_cookie,
            &token,
            "https://evil.invalid",
            &form,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_form(app, &appeal_url, author_cookie, &token, ORIGIN, &form)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post_form(app, &appeal_url, author_cookie, &token, ORIGIN, &form)
            .await
            .status(),
        StatusCode::SEE_OTHER,
        "same command key replays an existing appeal"
    );
    eprintln!("S049_PAID_APPEAL submission and replay succeeded");
    let appeal_id = scalar(
        url,
        &format!(
            "SELECT id FROM social_paid_comment_appeals
             WHERE hide_event_id={hide_event} AND author_user_id={author}"
        ),
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM social_paid_comment_appeals
                 WHERE hide_event_id={hide_event}"
            )
        ),
        1
    );
    // Keep the negative SQL test on the established disposable-PG fixture:
    // tor-web intentionally does not depend on sqlx directly.
    rejected_pg(
        url,
        &format!("DELETE FROM social_paid_comment_appeals WHERE id={appeal_id}"),
    );

    eprintln!("S049_PAID_APPEAL persisted author submission");
    let pending = read_page(app, admin_path, Some(&reviewer_cookie)).await;
    assert_eq!(pending.status(), StatusCode::OK);
    let pending_html = String::from_utf8(
        to_bytes(pending.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(pending_html.contains(&format!("Апелляция #{appeal_id}")));
    assert!(!pending_html.contains(BODY));
    assert!(!pending_html.contains("S049 paid private comment"));

    eprintln!("S049_PAID_APPEAL reviewer queue passed");
    let url_decision = format!("{admin_path}/{appeal_id}/decision");
    let admin_form =
        format!("csrf_token={token}&decision=restored&note=Independent+review+accepted");
    assert_eq!(
        post_form(
            app,
            &url_decision,
            original_admin_cookie,
            &token,
            ORIGIN,
            &admin_form,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "an administrator cannot reverse their own sanction"
    );
    assert_eq!(
        post_form(
            app,
            &url_decision,
            &reviewer_cookie,
            &token,
            ORIGIN,
            &admin_form
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    eprintln!("S049_PAID_APPEAL independent restore decision returned");
    assert_eq!(
        pg(
            url,
            &format!("SELECT visibility FROM discussion_comments WHERE id={comment}")
        ),
        "visible",
        "independent review can restore only the E comment"
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM social_paid_comment_appeal_decisions
                 WHERE appeal_id={appeal_id} AND reviewer_user_id={independent}
                 AND decision='restored'"
            )
        ),
        1
    );
    assert_eq!(
        post_form(
            app,
            &url_decision,
            &reviewer_cookie,
            &token,
            ORIGIN,
            &admin_form
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "appeal resolution is exactly once"
    );
    rejected_pg(
        url,
        &format!(
            "UPDATE social_paid_comment_appeal_decisions
             SET decision='denied' WHERE appeal_id={appeal_id}"
        ),
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM social_paid_comment_review_events
                 WHERE report_id={hide_report} AND reviewer_user_id={original_admin}
                   AND moderation_action='hide_paid_comment'"
            )
        ),
        1,
        "the original sanction remains immutable after independent appeal"
    );
    pg(
        url,
        &format!(
            "UPDATE account_roles SET is_active=false
             WHERE user_id={independent} AND role='administrator'"
        ),
    );
    assert_eq!(
        read_page(app, admin_path, Some(&reviewer_cookie))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}
