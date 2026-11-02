//! S005: one-click single-chapter wallet purchase through real HTTP, PG and S3.
use super::*;

fn hidden_field(html: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    let start = html.find(&marker).expect("published checkout field") + marker.len();
    let value = html[start..].split('"').next().unwrap();
    assert!(!value.is_empty(), "empty {name}");
    value.to_owned()
}

async fn post_checkout(
    app: &Router,
    chapter: i64,
    session: &str,
    csrf_cookie: &str,
    origin: &str,
    form: &str,
) -> Response {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/me/chapters/{chapter}/buy"))
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(
            header::COOKIE,
            format!("tor-dev-session={session}; tor-dev-csrf={csrf_cookie}"),
        )
        .body(Body::from(form.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

pub(super) async fn checkout_and_verify(
    app: &Router,
    url: &str,
    edition: (i64, i64),
    buyer: (i64, &str),
    owner_session: &str,
    paid_marker: &str,
) {
    let (chapter, version) = edition;
    let (actor, session) = buyer;
    assert_eq!(env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref(), Ok("1"));
    let path = format!("/me/chapters/{chapter}/buy");

    // The public paywall offers a real path only with the private S3 reader on.
    let paywall = private_get(app, &format!("/reader/{chapter}"), None).await;
    assert_eq!(paywall.status(), StatusCode::OK);
    assert_eq!(paywall.headers()["x-robots-tag"], "noindex, nofollow");
    let paywall_html = html_body(paywall).await;
    assert!(paywall_html.contains(&format!("href=\"{path}\"")));
    assert!(!paywall_html.contains(paid_marker));

    let guest = private_get(app, &path, None).await;
    assert_eq!(guest.status(), StatusCode::SEE_OTHER);
    assert_eq!(guest.headers()[header::LOCATION], "/login");
    let owner = private_get(app, &path, Some(owner_session)).await;
    assert_eq!(owner.status(), StatusCode::NOT_FOUND);

    let csrf = generate_session_token();
    let offer = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header(
                    header::COOKIE,
                    format!("tor-dev-session={session}; tor-dev-csrf={csrf}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(offer.status(), StatusCode::OK);
    assert!(offer.headers()[header::CACHE_CONTROL]
        .to_str()
        .unwrap()
        .contains("no-store"));
    assert_eq!(offer.headers()["x-robots-tag"], "noindex, nofollow");
    let html = html_body(offer).await;
    assert!(html.contains("Подтвердить покупку"));
    assert!(html.contains("1,75 ₽"));
    assert!(html.contains("&lt;img src=x onerror=window.__xss=true&gt;"));
    assert!(!html.contains("<img src=x onerror=window.__xss=true>"));
    assert!(!html.contains(paid_marker));
    assert!(!html.contains("X-Amz-Signature"));
    assert_eq!(
        hidden_field(&html, "expected_version_id"),
        version.to_string()
    );
    assert_eq!(hidden_field(&html, "expected_price_minor"), "175");
    assert_eq!(hidden_field(&html, "csrf_token"), csrf);
    let key = hidden_field(&html, "command_key");
    let form = format!(
        "csrf_token={csrf}&command_key={key}&expected_version_id={version}&expected_price_minor=175"
    );

    // A forged price, version, Origin or CSRF must not spend wallet funds.
    let stale = format!(
        "csrf_token={csrf}&command_key={key}&expected_version_id={version}&expected_price_minor=176"
    );
    assert_eq!(
        post_checkout(app, chapter, session, &csrf, ORIGIN, &stale)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let stale_version = format!(
        "csrf_token={csrf}&command_key={key}&expected_version_id={}&expected_price_minor=175",
        version + 1
    );
    assert_eq!(
        post_checkout(app, chapter, session, &csrf, ORIGIN, &stale_version)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        post_checkout(app, chapter, session, &csrf, "https://evil.invalid", &form)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let wrong_csrf = format!(
        "csrf_token=forged&command_key={key}&expected_version_id={version}&expected_price_minor=175"
    );
    assert_eq!(
        post_checkout(app, chapter, session, &csrf, ORIGIN, &wrong_csrf)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    // The form body is bounded independently of field validation or wallet logic.
    let oversized = format!("{form}&ignored={}", "X".repeat(2050));
    assert_eq!(
        post_checkout(app, chapter, session, &csrf, ORIGIN, &oversized)
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        scalar(
            url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={actor}")
        ),
        600
    );
    assert_eq!(
        scalar(
            url,
            &format!("SELECT count(*) FROM chapter_purchase_facts WHERE user_id={actor}")
        ),
        0
    );

    let first = post_checkout(app, chapter, session, &csrf, ORIGIN, &form).await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    let purchase = scalar(
        url,
        &format!(
            "SELECT purchase_id FROM library_purchased_chapters WHERE user_id={actor} AND chapter_id={chapter}"
        ),
    );
    let reading = format!("/read/paid/{chapter}?purchase_id={purchase}");
    assert_eq!(first.headers()[header::LOCATION], reading.as_str());
    assert_eq!(
        paid_page(app, chapter, purchase, session).await.status(),
        StatusCode::OK
    );
    let grant = post_grant(app, chapter, Some(session), Some(&csrf), ORIGIN, "{}").await;
    assert_eq!(grant.status(), StatusCode::OK);
    let grant_body = decode(grant).await;
    assert_eq!(grant_body["version_id"], version.to_string());
    assert!(!grant_body.to_string().contains(paid_marker));

    // Repeating the exact command is a replay, not a second debit.
    let replay = post_checkout(app, chapter, session, &csrf, ORIGIN, &form).await;
    assert_eq!(replay.status(), StatusCode::SEE_OTHER);
    assert_eq!(replay.headers()[header::LOCATION], reading.as_str());
    assert_eq!(
        scalar(
            url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={actor}")
        ),
        425
    );
    assert_eq!(
        scalar(
            url,
            &format!("SELECT count(*) FROM chapter_purchase_facts WHERE user_id={actor}")
        ),
        1
    );
    let owned = private_get(app, &path, Some(session)).await;
    assert_eq!(owned.status(), StatusCode::SEE_OTHER);
    assert_eq!(owned.headers()[header::LOCATION], "/me/purchases");
}
