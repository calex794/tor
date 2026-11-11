//! Valid old-client payloads and forged navigation cannot create new attribution.
use super::*;
async fn assert_retired(response: Response) {
    assert_eq!(response.status(), StatusCode::GONE);
    private(&response);
    let body: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(body["error"]["code"], "COLLECTION_RETIRED");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn valid_ticket_payloads_and_forged_navigation_cannot_restart_individual_attribution() {
    let h = setup().await;
    let (browser, reader) = registration(
        &h.app,
        &h.url,
        &format!("funnel-proof-{}@tor-ci.example", h.stamp),
    )
    .await;
    let (other, other_id) = registration(
        &h.app,
        &h.url,
        &format!("funnel-proof-other-{}@tor-ci.example", h.stamp),
    )
    .await;
    let item = book(&h.url, &format!("FLOW-proof-search-{}", h.stamp));
    assert_current_schema(&h.url);
    // Opaque, syntactically valid payloads from an old client. They are never
    // persisted as issued tickets on the current schema. Actual pre-barrier
    // assignments and their erasure are covered by the installed upgrade test.
    let rec_ticket = generate_session_token();
    let organic_ticket = generate_session_token();
    let before = optional_snapshot(&h.url, reader);
    let other_before = optional_snapshot(&h.url, other_id);

    let response = browser
        .get(&h.app, "/api/v1/recommendations?surface=home")
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    private(&response);
    let feed: Value = serde_json::from_str(&text(response).await).unwrap();
    assert!(feed["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["ticket"].is_null()));
    for path in [
        "/catalog?sort=newest".to_owned(),
        format!("/catalog?q=FLOW-proof-search-{}", h.stamp),
        format!("/books/{}", item.0),
        format!("/reader/{}", item.1),
    ] {
        let html = navigation::page_without_collection(&h, &browser, reader, &path).await;
        assert!(!html.contains("data-rec-ticket"));
        assert!(!html.contains("data-organic-ticket"));
    }
    let forged_path = format!(
        "/books/{}?source=recommendations&rec_assignment=00000000-0000-0000-0000-000000000000",
        item.0
    );
    assert_eq!(
        browser
            .request(
                &h.app,
                "GET",
                &forged_path,
                "",
                &[
                    ("x-flow-book-id", "9223372036854775807"),
                    ("x-flow-discovery-kind", "0"),
                ]
            )
            .await
            .status(),
        StatusCode::OK
    );

    let forged_ticket = "A".repeat(43);
    for (endpoint, ticket) in [
        ("/api/v1/recommendations/activate", rec_ticket.as_str()),
        ("/api/v1/discovery/activate", organic_ticket.as_str()),
        ("/api/v1/recommendations/activate", forged_ticket.as_str()),
    ] {
        for actor in [&browser, &other] {
            let body = format!(
                "ticket={ticket}&csrf_token={}",
                actor.cookies["tor-dev-csrf"]
            );
            assert_retired(actor.request(&h.app, "POST", endpoint, &body, &[]).await).await;
            let forbidden = actor
                .request(
                    &h.app,
                    "POST",
                    endpoint,
                    &body,
                    &[("origin", "https://attacker.invalid")],
                )
                .await;
            assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
            private(&forbidden);
            let forged = body.replace(&actor.cookies["tor-dev-csrf"], "wrong");
            let forbidden = actor.request(&h.app, "POST", endpoint, &forged, &[]).await;
            assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
            private(&forbidden);
        }
    }
    let claim = json!({"items":[{"ticket":rec_ticket,"visible_fraction_per_mille":500,
        "continuous_visible_ms":1000,"document_visible":true}]})
    .to_string();
    let csrf = browser.cookies["tor-dev-csrf"].as_str();
    assert_retired(
        browser
            .request(
                &h.app,
                "POST",
                "/api/v1/recommendations/impressions",
                &claim,
                &[("content-type", "application/json"), ("x-csrf-token", csrf)],
            )
            .await,
    )
    .await;
    let invalid = browser
        .request(
            &h.app,
            "POST",
            "/api/v1/recommendations/impressions",
            "{\"items\":[]}",
            &[("content-type", "application/json"), ("x-csrf-token", csrf)],
        )
        .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    private(&invalid);
    let invalid = browser
        .request(
            &h.app,
            "POST",
            "/api/v1/recommendations/activate",
            &format!("ticket=malformed&csrf_token={csrf}"),
            &[],
        )
        .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    private(&invalid);

    assert_eq!(
        optional_snapshot(&h.url, reader),
        before,
        "valid opaque payloads cannot be viewed, clicked, opened, delivered or retained"
    );
    assert_eq!(
        optional_snapshot(&h.url, other_id),
        other_before,
        "another valid session cannot create an analytics subject"
    );
    assert_eq!(
        pg(
            &h.url,
            &format!("SELECT count(*) FROM reader_chapter_starts WHERE user_id={reader}")
        ),
        "0"
    );
    pg(
        &h.url,
        &format!(
            "DELETE FROM rec_subjects WHERE user_id={reader};
        UPDATE books SET admin_hold=true WHERE id={}",
            item.0
        ),
    );
}
