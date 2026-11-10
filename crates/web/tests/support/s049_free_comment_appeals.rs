//! S049: real PostgreSQL/HTTP continuation of a manually hidden free E comment.
use super::*;

pub(super) async fn check(
    url: &str,
    app: &Router,
    sanctioned: (i64, i64, i64),
    credentials: (&str, &str, &str, &str),
) {
    let (comment, hide_report_id, original_admin) = sanctioned;
    let (author_cookie, original_admin_cookie, moderator_cookie, csrf) = credentials;
    let hide_event_id: i64 = pg(
        url,
        &format!(
            "SELECT id FROM social_free_comment_review_events
             WHERE report_id={hide_report_id} AND moderation_action='hide_free_comment'"
        ),
    )
    .parse()
    .unwrap();
    let author_id: i64 = pg(
        url,
        &format!("SELECT author_user_id FROM discussion_comments WHERE id={comment}"),
    )
    .parse()
    .unwrap();

    let catalog = PgCatalog::connect(url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let reviewer_session = generate_session_token();
    let independent_admin = accounts
        .register_with_session(
            &format!("s049-free-appeal-reviewer-{stamp}@example.invalid"),
            "IndependentFreeReview",
            &hash_password("S049FreeAppealReviewerStrongPassword").unwrap(),
            &session_token_digest(&reviewer_session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    pg(
        url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({independent_admin},'administrator',TRUE)"
        ),
    );
    let reviewer_cookie = format!("tor-dev-session={reviewer_session}; tor-dev-csrf={csrf}");

    let author_path = "/me/free-comment-appeals";
    let admin_path = "/admin/free-comment-appeals";
    assert_eq!(
        request(app, "GET", author_path, "", ORIGIN, "")
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(
        request(app, "GET", admin_path, moderator_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN,
    );
    let mine = request(app, "GET", author_path, author_cookie, ORIGIN, "").await;
    assert_eq!(mine.status(), StatusCode::OK);
    assert_eq!(mine.headers()[header::CACHE_CONTROL], "no-store");
    let mine_html = String::from_utf8(
        to_bytes(mine.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(mine_html.contains(&format!("решение #{hide_event_id}")));
    assert!(!mine_html.contains("S049_FREE_CHAPTER_BODY_NOT_REPORT"));

    let body = format!(
        "csrf_token={csrf}&command_key={}&reason=Request+independent+review+of+my+hidden+free+comment",
        "A".repeat(43)
    );
    let submit_path = format!("{author_path}/{hide_event_id}");
    assert_eq!(
        request(app, "POST", &submit_path, moderator_cookie, ORIGIN, &body)
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "a moderator cannot appeal somebody else's comment",
    );
    assert_eq!(
        request(
            app,
            "POST",
            &submit_path,
            author_cookie,
            "https://evil.invalid",
            &body
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        request(app, "POST", &submit_path, author_cookie, ORIGIN, &body)
            .await
            .status(),
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        request(app, "POST", &submit_path, author_cookie, ORIGIN, &body)
            .await
            .status(),
        StatusCode::SEE_OTHER,
        "the same command key is idempotent",
    );
    let appeal_id: i64 = pg(
        url,
        &format!(
            "SELECT id FROM social_free_comment_appeals
             WHERE hide_event_id={hide_event_id} AND author_user_id={author_id}"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM social_free_comment_appeals
                 WHERE hide_event_id={hide_event_id}"
            ),
        ),
        "1",
    );
    pg_rejected(
        url,
        &format!("DELETE FROM social_free_comment_appeals WHERE id={appeal_id}"),
    );
    let pending = request(app, "GET", admin_path, &reviewer_cookie, ORIGIN, "").await;
    assert_eq!(pending.status(), StatusCode::OK);
    let pending_html = String::from_utf8(
        to_bytes(pending.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(pending_html.contains(&format!("Апелляция #{appeal_id}")));
    assert!(!pending_html.contains("S049_FREE_CHAPTER_BODY_NOT_REPORT"));
    assert!(!pending_html.contains("E public free comment"));

    let decide_path = format!("{admin_path}/{appeal_id}/decision");
    let decision =
        format!("csrf_token={csrf}&decision=restored&note=Independent+free+comment+review");
    assert_eq!(
        request(
            app,
            "POST",
            &decide_path,
            original_admin_cookie,
            ORIGIN,
            &decision
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "the original sanction reviewer cannot decide its appeal",
    );
    assert_eq!(
        request(
            app,
            "POST",
            &decide_path,
            moderator_cookie,
            ORIGIN,
            &decision
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        request(
            app,
            "POST",
            &decide_path,
            &reviewer_cookie,
            ORIGIN,
            &decision
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        pg(
            url,
            &format!("SELECT visibility FROM discussion_comments WHERE id={comment}"),
        ),
        "visible",
        "independent reviewer restores the exact comment and nothing else",
    );
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM social_free_comment_appeal_decisions
                 WHERE appeal_id={appeal_id}
                   AND reviewer_user_id={independent_admin} AND decision='restored'"
            ),
        ),
        "1",
    );
    assert_eq!(
        request(
            app,
            "POST",
            &decide_path,
            &reviewer_cookie,
            ORIGIN,
            &decision
        )
        .await
        .status(),
        StatusCode::CONFLICT,
    );
    pg_rejected(
        url,
        &format!(
            "UPDATE social_free_comment_appeal_decisions SET decision='denied'
             WHERE appeal_id={appeal_id}"
        ),
    );
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM social_free_comment_review_events
                 WHERE report_id={hide_report_id} AND reviewer_user_id={original_admin}
                   AND moderation_action='hide_free_comment'"
            ),
        ),
        "1",
    );
    pg(
        url,
        &format!(
            "UPDATE account_roles SET is_active=FALSE
             WHERE user_id={independent_admin} AND role='administrator'"
        ),
    );
    assert_eq!(
        request(app, "GET", admin_path, &reviewer_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::FORBIDDEN,
    );
}
