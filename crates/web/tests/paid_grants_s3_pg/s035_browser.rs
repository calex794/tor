//! S035 native two-step HTTP checkout for two actual private S3 editions.
use super::*;

fn hidden_field(html: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    let start = html.find(&marker).expect("confirmation field") + marker.len();
    let value = html[start..].split('"').next().unwrap();
    assert!(!value.is_empty());
    value.to_owned()
}

async fn native_batch_post(
    app: &Router,
    book: i64,
    session: &str,
    csrf: &str,
    form: &str,
) -> Response {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/me/books/{book}/buy-chapters"))
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(
            header::COOKIE,
            format!("tor-dev-session={session}; tor-dev-csrf={csrf}"),
        )
        .body(Body::from(form.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

pub(super) async fn checkout_and_verify(
    app: &Router,
    url: &str,
    book: i64,
    chapter: i64,
    chapter_two: i64,
    buyer: i64,
    buyer_cookie: &str,
    csrf: &str,
    paid_marker: &str,
) -> (i64, i64) {
    // S035 browser-facing HTTP: select both offers, see the aggregate quote,
    // then buy both real encrypted editions through one wallet transaction.
    assert_eq!(env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref(), Ok("1"));
    let selection = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/me/books/{book}/buy-chapters"))
                .header(
                    header::COOKIE,
                    format!("tor-dev-session={buyer_cookie}; tor-dev-csrf={csrf}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selection.status(), StatusCode::OK);
    let selection_html = html_body(selection).await;
    for chapter_id in [chapter, chapter_two] {
        assert!(selection_html.contains(&format!("name=\"chapter_{chapter_id}\"")));
    }
    assert!(!selection_html.contains(paid_marker));
    let choose = format!(
        "step=select&csrf_token={csrf}&chapter_{chapter}=1&chapter_{chapter_two}=1"
    );
    let quote = native_batch_post(app, book, &buyer_cookie, &csrf, &choose).await;
    assert_eq!(quote.status(), StatusCode::OK);
    let quote_html = html_body(quote).await;
    assert!(quote_html.contains("Итого: 3,50 ₽"));
    assert!(quote_html.contains("Подтвердить покупку"));
    assert_eq!(
        scalar(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")
        ),
        600
    );
    let form = format!(
        "step=commit&csrf_token={csrf}&command_key={}&chapter_ids={}&version_ids={}&prices={}",
        hidden_field(&quote_html, "command_key"),
        hidden_field(&quote_html, "chapter_ids"),
        hidden_field(&quote_html, "version_ids"),
        hidden_field(&quote_html, "prices")
    );
    let purchased = native_batch_post(app, book, &buyer_cookie, &csrf, &form).await;
    assert_eq!(purchased.status(), StatusCode::OK);
    let receipt_html = html_body(purchased).await;
    assert!(receipt_html.contains("Покупка завершена"));
    assert!(receipt_html.contains("2 глав куплено"));
    assert!(!receipt_html.contains(paid_marker));
    let purchase_id = scalar(
        &url,
        &format!(
            "SELECT purchase_id FROM library_purchased_chapters WHERE user_id={buyer} AND chapter_id={chapter}"
        ),
    );
    let purchase_two_id = scalar(
        &url,
        &format!(
            "SELECT purchase_id FROM library_purchased_chapters WHERE user_id={buyer} AND chapter_id={chapter_two}"
        ),
    );
    assert_ne!(purchase_id, purchase_two_id);
    assert!(receipt_html.contains(&format!(
        "/read/paid/{chapter}?purchase_id={purchase_id}"
    )));
    assert!(receipt_html.contains(&format!(
        "/read/paid/{chapter_two}?purchase_id={purchase_two_id}"
    )));
    let replay = native_batch_post(app, book, &buyer_cookie, &csrf, &form).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert!(html_body(replay).await.contains("Повтор без нового списания"));
    assert_eq!(
        scalar(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")
        ),
        250
    );
    assert_eq!(
        scalar(
            &url,
            &format!("SELECT count(*) FROM chapter_purchase_facts WHERE user_id={buyer}")
        ),
        2
    );
    (purchase_id, purchase_two_id)
}
