//! S034/S031: write one disposable, mode-0600 browser fixture for direct and Layer readers.
use super::*;

// Capture the real static route, not a second implementation of bundle assembly.
pub(super) async fn paid_reader_script(app: &Router) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/assets/newboo-paid-reader.mjs")
                .header(header::HOST, "127.0.0.1:3098")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/javascript; charset=utf-8"
    );
    String::from_utf8(
        to_bytes(response.into_body(), 512 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

pub(super) fn save_ephemeral_fixture(
    grant: Value,
    version: i64,
    purchase: i64,
    direct_reader: (String, String, String),
    layer_reader: (String, Value, String),
    expiry_probe_url: Value,
    toc_target: (i64, i64, i64, i64, i64),
) {
    let (reader_html, reader_csp, reader_script) = direct_reader;
    let (layer_reader_html, layer_grant, layer_reader_csp) = layer_reader;
    let target = env::var("TOR_PAID_GRANT_FIXTURE_PATH").expect("ephemeral fixture output");
    let runner_temp = env::var("RUNNER_TEMP").expect("CI temp directory");
    assert!(
        Path::new(&target).starts_with(&runner_temp),
        "never store scoped secrets inside the source checkout"
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&target).expect("private fixture output");
    let payload = json!({
        "grant": grant,
        "purchased_version_id": version.to_string(),
        "purchase_id": purchase.to_string(),
        "reader_html": reader_html,
        "reader_csp": reader_csp,
        "reader_script": reader_script,
        "layer_reader_html": layer_reader_html,
        "layer_grant": layer_grant,
        "layer_reader_csp": layer_reader_csp,
        "expiry_probe_url": expiry_probe_url,
        "expected_markdown": MARKDOWN,
        "expected_layer_markdown": "New V2, not purchased",
        "expected_book_title": BOOK_TITLE,
        "expected_chapter_title": CHAPTER_TITLE,
        "expected_chapter_no": 1,
        "expected_version_no": 1,
        "toc_second_chapter_id": toc_target.0.to_string(),
        "toc_second_purchase_id": toc_target.1.to_string(),
        "toc_second_title": CHAPTER_TWO_TITLE,
        "toc_free_chapter_id": toc_target.2.to_string(),
        "toc_locked_chapter_id": toc_target.3.to_string(),
        "toc_draft_chapter_id": toc_target.4.to_string(),
    });
    file.write_all(payload.to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
}
