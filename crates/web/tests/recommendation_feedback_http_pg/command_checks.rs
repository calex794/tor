//! Guarded compatibility contracts; no retired request acknowledges a write.
use super::*;

pub(super) async fn cached_clients(
    app: &Router,
    cookie: &str,
    other_cookie: &str,
    csrf: &str,
    other: i64,
) {
    let command = generate_session_token();
    let enable = format!("enabled=1&expected_revision=7&command_key={command}&csrf_token={csrf}");
    for endpoint in [
        "/me/recommendations/consent",
        "/me/recommendations/flow-consent",
    ] {
        let rejected = call(
            app,
            "POST",
            endpoint,
            cookie,
            ("https://evil.invalid", csrf),
            &enable,
            false,
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
        private(&rejected);
        assert_retired(
            call(
                app,
                "POST",
                endpoint,
                cookie,
                (ORIGIN, csrf),
                &enable,
                false,
            )
            .await,
        )
        .await;
    }
    for endpoint in [
        "/me/recommendations/consent",
        "/me/recommendations/flow-consent",
    ] {
        for (revision, key) in [
            (-1, command.as_str()),
            (i64::MAX, command.as_str()),
            (7, "bad"),
        ] {
            let payload = format!(
                "enabled=1&expected_revision={revision}&command_key={key}&csrf_token={csrf}"
            );
            let rejected = call(
                app,
                "POST",
                endpoint,
                cookie,
                (ORIGIN, csrf),
                &payload,
                false,
            )
            .await;
            assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
            private(&rejected);
        }
    }
    let ticket = generate_session_token();
    let activate = format!("ticket={ticket}&csrf_token={csrf}");
    for endpoint in [
        "/api/v1/recommendations/activate",
        "/api/v1/discovery/activate",
    ] {
        for session_cookie in [cookie, other_cookie] {
            assert_retired(
                call(
                    app,
                    "POST",
                    endpoint,
                    session_cookie,
                    (ORIGIN, csrf),
                    &activate,
                    false,
                )
                .await,
            )
            .await;
        }
        let guest_cookie = format!("tor-dev-csrf={csrf}");
        let denied = call(
            app,
            "POST",
            endpoint,
            &guest_cookie,
            (ORIGIN, csrf),
            &activate,
            false,
        )
        .await;
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        private(&denied);
        let denied = call(
            app,
            "POST",
            endpoint,
            cookie,
            (ORIGIN, csrf),
            &format!("ticket={ticket}&csrf_token=invalid"),
            false,
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        private(&denied);
    }
    let visible = json!({"items":[{"ticket":ticket,"visible_fraction_per_mille":500,
        "continuous_visible_ms":1000,"document_visible":true}]})
    .to_string();
    assert_retired(
        call(
            app,
            "POST",
            "/api/v1/recommendations/impressions",
            cookie,
            (ORIGIN, csrf),
            &visible,
            true,
        )
        .await,
    )
    .await;
    let rejected = call(
        app,
        "POST",
        "/api/v1/recommendations/impressions",
        cookie,
        (ORIGIN, "wrong"),
        &visible,
        true,
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    private(&rejected);
    let claim = json!({"ticket":ticket,"visible_fraction_per_mille":500,
        "continuous_visible_ms":1000,"document_visible":true});
    for claims in [Vec::<Value>::new(), vec![claim.clone(); 21]] {
        let rejected = call(
            app,
            "POST",
            "/api/v1/recommendations/impressions",
            cookie,
            (ORIGIN, csrf),
            &json!({"items":claims}).to_string(),
            true,
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        private(&rejected);
    }
    for (field, value) in [
        ("ticket", json!("malformed")),
        ("visible_fraction_per_mille", json!(1001)),
        ("continuous_visible_ms", json!(60001)),
    ] {
        let mut invalid = claim.clone();
        invalid[field] = value;
        let rejected = call(
            app,
            "POST",
            "/api/v1/recommendations/impressions",
            cookie,
            (ORIGIN, csrf),
            &json!({"items":[invalid]}).to_string(),
            true,
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        private(&rejected);
    }
    let extra = json!({"items":[claim],"user_id":other}).to_string();
    let rejected = call(
        app,
        "POST",
        "/api/v1/recommendations/impressions",
        cookie,
        (ORIGIN, csrf),
        &extra,
        true,
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    private(&rejected);
    for endpoint in [
        "/api/v1/recommendations/activate",
        "/api/v1/discovery/activate",
    ] {
        let rejected = call(
            app,
            "POST",
            endpoint,
            cookie,
            (ORIGIN, csrf),
            &format!("ticket=malformed&csrf_token={csrf}"),
            false,
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        private(&rejected);
        let rejected = call(
            app,
            "POST",
            endpoint,
            cookie,
            (ORIGIN, csrf),
            &format!("ticket={}&csrf_token={csrf}", "A".repeat(40_000)),
            false,
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::PAYLOAD_TOO_LARGE);
        private(&rejected);
    }
}

pub(super) async fn administrator(
    app: &Router,
    cookie: &str,
    csrf: &str,
    url: &str,
    user: i64,
    book: i64,
) {
    pg(
        url,
        &format!("INSERT INTO account_roles(user_id,role) VALUES({user},'administrator')"),
    );
    let managed = call(
        app,
        "GET",
        "/me/recommendations/manage",
        cookie,
        (ORIGIN, csrf),
        "",
        false,
    )
    .await;
    assert_eq!(managed.status(), StatusCode::OK);
    private(&managed);
    let managed = text(managed).await;
    assert!(managed.contains("recommendation-model-retired"));
    // The management list is bounded and ordered by ascending book ID.
    // This fixture may be outside its first page on the shared CI database.
    assert!(managed.contains("href=\"/me/books/"));
    let direct = call(
        app,
        "GET",
        &format!("/me/books/{book}/recommendations"),
        cookie,
        (ORIGIN, csrf),
        "",
        false,
    )
    .await;
    assert_eq!(direct.status(), StatusCode::OK);
    private(&direct);
    assert!(!managed.contains("name=\"action\""));
    let policy_before = pg(
        url,
        "SELECT row_to_json(p) FROM rec_runtime_policy p WHERE singleton",
    );
    for query in [
        "",
        "?channel=organic&days=180",
        "?channel=recommendation&days=30",
    ] {
        assert_retired(
            call(
                app,
                "GET",
                &format!("/api/v1/admin/recommendations/quality{query}"),
                cookie,
                (ORIGIN, csrf),
                "",
                false,
            )
            .await,
        )
        .await;
    }
    for query in [
        "?channel=bad",
        "?channel=organic&days=181",
        "?days=0",
        "?days=31",
    ] {
        let rejected = call(
            app,
            "GET",
            &format!("/api/v1/admin/recommendations/quality{query}"),
            cookie,
            (ORIGIN, csrf),
            "",
            false,
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        private(&rejected);
    }
    let control = json!({"action":"hold","expected_revision":0}).to_string();
    assert_retired(
        call(
            app,
            "POST",
            "/api/v1/admin/recommendations/control",
            cookie,
            (ORIGIN, csrf),
            &control,
            true,
        )
        .await,
    )
    .await;
    assert_retired(
        call(
            app,
            "POST",
            "/me/recommendations/manage",
            cookie,
            (ORIGIN, csrf),
            &format!("action=hold&expected_revision=0&csrf_token={csrf}"),
            false,
        )
        .await,
    )
    .await;
    for payload in [
        json!({"action":"hold","expected_revision":-1}),
        json!({"action":"activate","expected_revision":0,"generation":"not-uuid","percent":100}),
        json!({"action":"hold","expected_revision":0,"percent":99}),
    ] {
        let rejected = call(
            app,
            "POST",
            "/api/v1/admin/recommendations/control",
            cookie,
            (ORIGIN, csrf),
            &payload.to_string(),
            true,
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        private(&rejected);
    }
    assert_eq!(
        pg(
            url,
            "SELECT row_to_json(p) FROM rec_runtime_policy p WHERE singleton"
        ),
        policy_before
    );
    pg(url, &format!("UPDATE account_roles SET is_active=false WHERE user_id={user} AND role='administrator'"));
    for path in [
        "/api/v1/admin/recommendations/quality?channel=invalid",
        "/api/v1/admin/recommendations/quality?channel=organic",
        "/me/recommendations/manage",
    ] {
        let rejected = call(app, "GET", path, cookie, (ORIGIN, csrf), "", false).await;
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
        private(&rejected);
    }
}
