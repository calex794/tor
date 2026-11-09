//! S049: real PG/HTTP acceptance of manual administrator-only paid-comment reviews.
use super::*;
#[path = "s049_paid_comment_appeals.rs"]
mod paid_appeals;

async fn decision(
    app: &Router,
    report: i64,
    session: &str,
    csrf: &str,
    origin: &str,
    action: &str,
) -> Response {
    let form = format!("csrf_token={csrf}&decision={action}&note=Manual+review+accepted");
    let req = Request::builder()
        .uri(format!("/admin/paid-comment-reports/{report}/decision"))
        .method("POST")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ORIGIN, origin)
        .header(
            header::COOKIE,
            format!("tor-dev-session={session}; tor-dev-csrf={csrf}"),
        )
        .body(Body::from(form))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

fn rejected_pg(url: &str, query: &str) {
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_ci"));
    let result = Command::new("psql")
        .args([
            "-X",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "--command",
            query,
        ])
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "immutable decision must reject updates"
    );
}

pub(super) async fn check(
    url: &str,
    accounts: &PgAccounts,
    app: &Router,
    buyer: i64,
    chapter: i64,
    author_cookie: &str,
) {
    let (admin, admin_cookie) = account(accounts, "s049-paid-comment-admin@tor-ci.example").await;
    let (moderator, moderator_cookie) =
        account(accounts, "s049-paid-comment-moderator@tor-ci.example").await;
    pg(
        url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
         VALUES({admin},'administrator',TRUE),({moderator},'moderator',TRUE)
         ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE"
        ),
    );
    let report = scalar(
        url,
        &format!(
            "SELECT id FROM social_paid_comment_reports
             WHERE reporter_user_id={buyer} AND chapter_id={chapter}
               AND state='pending' ORDER BY id DESC LIMIT 1"
        ),
    );
    let comment = scalar(
        url,
        &format!("SELECT comment_id FROM social_paid_comment_reports WHERE id={report}"),
    );
    pg(
        url,
        &format!("UPDATE discussion_comments SET visibility='visible' WHERE id={comment}"),
    );

    let path = "/admin/paid-comment-reports";
    assert_eq!(
        read_page(app, path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        read_page(app, path, Some(&moderator_cookie)).await.status(),
        StatusCode::FORBIDDEN,
        "Book registration moderator cannot inspect paid comment complaints"
    );
    let pending = read_page(app, path, Some(&admin_cookie)).await;
    assert_eq!(pending.status(), StatusCode::OK);
    assert_eq!(pending.headers()[header::CACHE_CONTROL], "no-store");
    let html = String::from_utf8(
        to_bytes(pending.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains(&format!("Жалоба #{report}")));
    assert!(html.contains("S049 paid private comment"));
    assert!(!html.contains(BODY));
    assert!(!html.contains("ciphertext_url"));
    assert!(!html.contains("dek_b64u"));
    let csrf = generate_session_token();

    assert_eq!(
        decision(app, report, &moderator_cookie, &csrf, ORIGIN, "reviewed")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        decision(
            app,
            report,
            &admin_cookie,
            &csrf,
            "https://bad.invalid",
            "reviewed"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        decision(app, report, &admin_cookie, &csrf, ORIGIN, "remove_chapter")
            .await
            .status(),
        StatusCode::BAD_REQUEST,
        "explicit paid comment hide is reserved for the second complaint"
    );
    let accept = decision(app, report, &admin_cookie, &csrf, ORIGIN, "reviewed").await;
    assert_eq!(accept.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM social_paid_comment_review_events
             WHERE report_id={report} AND reviewer_user_id={admin}
               AND decision='reviewed'"
            )
        ),
        1
    );
    assert_eq!(
        pg(
            url,
            &format!("SELECT state FROM social_paid_comment_reports WHERE id={report}")
        ),
        "reviewed"
    );
    assert_eq!(
        pg(
            url,
            &format!("SELECT visibility FROM discussion_comments WHERE id={comment}")
        ),
        "visible",
        "review never hides an edition comment automatically"
    );
    assert_eq!(
        decision(app, report, &admin_cookie, &csrf, ORIGIN, "dismissed")
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM social_paid_comment_review_events WHERE report_id={report}"
            )
        ),
        1,
        "a decided report has exactly one immutable decision"
    );
    rejected_pg(
        url,
        &format!(
            "UPDATE social_paid_comment_review_events
             SET decision='dismissed' WHERE report_id={report}"
        ),
    );
    let history = read_page(app, path, Some(&admin_cookie)).await;
    assert_eq!(history.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(history.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains(&format!("Решение по жалобе #{report}")));
    assert!(!body.contains(BODY));

    // A second report for the same exact E comment requires a different
    // command key and an explicit, auditable administrator hide decision.
    let second = scalar(
        url,
        &format!(
            "INSERT INTO social_paid_comment_reports
             (chapter_id,version_id,comment_id,reporter_user_id,reason,details,command_key)
             SELECT chapter_id,version_id,comment_id,reporter_user_id,
                    'spam','Second complaint','{}'
             FROM social_paid_comment_reports WHERE id={report} RETURNING id",
            "X".repeat(43)
        ),
    );
    assert_eq!(
        decision(
            app,
            second,
            &moderator_cookie,
            &csrf,
            ORIGIN,
            "hide_comment"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        decision(app, second, &admin_cookie, &csrf, ORIGIN, "hide_comment")
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            url,
            &format!("SELECT visibility FROM discussion_comments WHERE id={comment}")
        ),
        "hidden",
        "manual sanction hides the precise E comment"
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM social_paid_comment_review_events
                 WHERE report_id={second} AND reviewer_user_id={admin}
                   AND decision='reviewed' AND moderation_action='hide_paid_comment'"
            ),
        ),
        1,
        "hide and its immutable audit event are one transaction"
    );
    assert_eq!(
        decision(app, second, &admin_cookie, &csrf, ORIGIN, "hide_comment")
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let hidden_history = read_page(app, path, Some(&admin_cookie)).await;
    let history_body = String::from_utf8(
        to_bytes(hidden_history.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(history_body.contains("hide_paid_comment"));
    assert!(!history_body.contains(BODY));
    eprintln!("S049_PAID_APPEAL start independent review");
    tokio::time::timeout(
        std::time::Duration::from_secs(45),
        paid_appeals::check(
            url,
            accounts,
            app,
            author_cookie,
            (comment, second, admin),
            &admin_cookie,
        ),
    )
    .await
    .expect("S049 paid comment appeal acceptance exceeded 45s");
    eprintln!("S049_PAID_APPEAL finished independent review");
    rejected_pg(
        url,
        &format!(
            "UPDATE social_paid_comment_review_events
             SET moderation_action='none' WHERE report_id={second}"
        ),
    );
    pg(
        url,
        &format!(
            "UPDATE account_roles SET is_active=FALSE WHERE user_id={admin}
         AND role='administrator'"
        ),
    );
    assert_eq!(
        read_page(app, path, Some(&admin_cookie)).await.status(),
        StatusCode::FORBIDDEN,
        "revoked administrator loses paid-comment complaint history immediately"
    );
    pg(
        url,
        &format!(
            "UPDATE account_roles SET is_active=FALSE WHERE user_id={moderator}
         AND role='moderator'"
        ),
    );
}
