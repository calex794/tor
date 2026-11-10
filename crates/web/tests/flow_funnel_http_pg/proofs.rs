use super::*;

fn organic_ticket(html: &str, book: i64) -> String {
    let target = format!("href=\"/books/{book}\"");
    html.split('<')
        .find(|tag| tag.contains(&target) && tag.contains("data-organic-ticket="))
        .expect("actual rendered result has a server ticket")
        .split("data-organic-ticket=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

async fn rec_ticket(browser: &Browser, app: &Router, book: i64) -> String {
    let response = browser
        .get(app, "/api/v1/recommendations?surface=home")
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let feed: Value = serde_json::from_str(&text(response).await).unwrap();
    feed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["book_id"].as_str() == Some(book.to_string().as_str()))
        .expect("real fallback feed includes the fixture")["ticket"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn activate(browser: &Browser, app: &Router, ticket: &str, organic: bool) {
    let endpoint = if organic {
        "/api/v1/discovery/activate"
    } else {
        "/api/v1/recommendations/activate"
    };
    let response = browser
        .request(
            app,
            "POST",
            endpoint,
            &format!(
                "ticket={ticket}&csrf_token={}",
                browser.cookies["tor-dev-csrf"]
            ),
            &[],
        )
        .await;
    assert_eq!(
        response.status(),
        if organic {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::SEE_OTHER
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn discovery_requires_real_current_session_proof_and_keeps_unknown_honest() {
    let h = setup().await;
    let (browser, reader) = registration(
        &h.app,
        &h.url,
        &format!("funnel-proof-{}@tor-ci.example", h.stamp),
    )
    .await;
    let hold = pg(
        &h.url,
        "SELECT hold FROM rec_runtime_policy WHERE singleton",
    ) == "t";
    pg(
        &h.url,
        "UPDATE rec_runtime_policy SET hold=true WHERE singleton",
    );
    let before = book(&h.url, "FLOW proof preconsent");
    let catalog = book(&h.url, "FLOW proof catalog");
    let search_title = format!("FLOW-proof-search-{}", h.stamp);
    let search = book(&h.url, &search_title);
    let rec = book(&h.url, "FLOW proof recommendation");
    let forged = book(&h.url, "FLOW proof forged");
    let stale = book(&h.url, "FLOW proof stale");
    set_choice(&browser, &h.app, REC_FORM, true).await;
    let ticket = rec_ticket(&browser, &h.app, before.0).await;
    activate(&browser, &h.app, &ticket, false).await;
    set_choice(&browser, &h.app, FLOW_FORM, true).await;
    assert_eq!(
        browser
            .get(&h.app, &format!("/books/{}", before.0))
            .await
            .status(),
        StatusCode::OK
    );
    wait_pages(&h.url, reader, 1).await;

    let page = browser.get(&h.app, "/catalog?sort=newest").await;
    assert_eq!(page.status(), StatusCode::OK);
    let ticket = organic_ticket(&text(page).await, catalog.0);
    activate(&browser, &h.app, &ticket, true).await;
    assert_eq!(
        browser
            .get(&h.app, &format!("/books/{}", catalog.0))
            .await
            .status(),
        StatusCode::OK
    );
    wait_pages(&h.url, reader, 3).await;
    let page = browser
        .get(&h.app, &format!("/catalog?q={search_title}"))
        .await;
    assert_eq!(page.status(), StatusCode::OK);
    let ticket = organic_ticket(&text(page).await, search.0);
    activate(&browser, &h.app, &ticket, true).await;
    assert_eq!(
        browser
            .get(&h.app, &format!("/books/{}", search.0))
            .await
            .status(),
        StatusCode::OK
    );
    wait_pages(&h.url, reader, 5).await;
    let ticket = rec_ticket(&browser, &h.app, rec.0).await;
    activate(&browser, &h.app, &ticket, false).await;
    assert_eq!(
        browser
            .get(&h.app, &format!("/books/{}", rec.0))
            .await
            .status(),
        StatusCode::OK
    );
    wait_pages(&h.url, reader, 6).await;

    let response = browser
        .request(
            &h.app,
            "POST",
            "/api/v1/recommendations/activate",
            &format!(
                "ticket={}&csrf_token={}",
                "A".repeat(43),
                browser.cookies["tor-dev-csrf"]
            ),
            &[],
        )
        .await;
    assert!(!response.status().is_success() && response.status() != StatusCode::SEE_OTHER);
    let path=format!("/books/{}?source=recommendations&book_id={}&rec_assignment=00000000-0000-0000-0000-000000000000",forged.0,rec.0);
    assert_eq!(
        browser
            .request(
                &h.app,
                "GET",
                &path,
                "",
                &[
                    ("x-flow-book-id", &rec.0.to_string()),
                    ("x-flow-discovery-kind", "0")
                ]
            )
            .await
            .status(),
        StatusCode::OK
    );
    wait_pages(&h.url, reader, 7).await;

    let page = browser.get(&h.app, "/catalog?sort=newest").await;
    let ticket = organic_ticket(&text(page).await, stale.0);
    activate(&browser, &h.app, &ticket, true).await;
    // Age an actually accepted navigation proof past its valid 15-minute age.
    pg(
        &h.url,
        &format!(
            "UPDATE organic_discovery_touches SET created_at=created_at-interval '16 minutes',
      expires_at=expires_at-interval '16 minutes',clicked_at=clicked_at-interval '16 minutes'
      WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id={reader}) AND book_id={}",
            stale.0
        ),
    );
    assert_eq!(
        browser
            .get(&h.app, &format!("/books/{}", stale.0))
            .await
            .status(),
        StatusCode::OK
    );
    wait_pages(&h.url, reader, 9).await;

    let expected = [
        (before.0, 3),
        (catalog.0, 1),
        (search.0, 2),
        (rec.0, 0),
        (forged.0, 3),
        (stale.0, 3),
    ];
    for (book, kind) in expected {
        assert_eq!(
            pg(
                &h.url,
                &format!(
                    "SELECT discovery_kind FROM flow_visit_receipts r
          JOIN rec_subjects s USING(subject_id) WHERE s.user_id={reader} AND book_id={book}"
                )
            ),
            kind.to_string()
        );
    }
    age_facts(&h.url, reader);
    settle_funnel(&h.funnel, &h.url).await;
    for (book, kind) in expected {
        assert_eq!(
            pg(
                &h.url,
                &format!(
                    "SELECT discovery_kind,started,completed FROM flow_book_cohorts c
          JOIN rec_subjects s USING(subject_id) WHERE s.user_id={reader} AND book_id={book}"
                )
            ),
            format!("{kind}|f|f")
        );
    }
    set_choice(&browser, &h.app, FLOW_FORM, false).await;
    pg(
        &h.url,
        &format!("UPDATE rec_runtime_policy SET hold={hold} WHERE singleton"),
    );
    let ids = expected.map(|(id, _)| id.to_string()).join(",");
    pg(
        &h.url,
        &format!("UPDATE books SET admin_hold=true WHERE id IN({ids})"),
    );
}
