#[cfg(test)]
mod cases {
    use super::super::*;
    use axum::{middleware, routing::get, Extension, Router};
    use std::sync::{Arc, Mutex};
    use tor_contracts::exposure::{CardObservation, ObservationIssuer};
    use tower::ServiceExt;

    const SCRIPT: &str = "<script defer src=\"/assets/flow-observations.js\"></script>";
    const HTML: &str = "<html><body><h1>/assets/flow-observations.js</h1><article data-flow-proof=\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\">Book</article></body></html>";

    struct TestIssuer;
    impl ObservationIssuer for TestIssuer {
        fn issue(&self, _: CardObservation) -> Option<String> {
            None
        }
    }

    async fn apply(response: Response) -> Response {
        let response = Arc::new(Mutex::new(Some(response)));
        let issuer: Arc<dyn ObservationIssuer> = Arc::new(TestIssuer);
        Router::new()
            .route(
                "/",
                get(move || {
                    let response = response.lock().unwrap().take().unwrap();
                    async move { response }
                }),
            )
            .layer(middleware::from_fn(append_script))
            .layer(Extension(issuer))
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn response(body: Body) -> Response {
        Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "public, max-age=600")
            .header(header::ETAG, "\"shared-html\"")
            .body(body)
            .unwrap()
    }

    fn isolated(response: &Response) {
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!response.headers().contains_key(header::ETAG));
    }

    #[tokio::test]
    async fn title_containing_asset_path_cannot_suppress_script_or_cache_isolation() {
        let mut input = response(Body::from(HTML));
        input.headers_mut().insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&HTML.len().to_string()).unwrap(),
        );
        let output = apply(input).await;
        isolated(&output);
        // Axum may regenerate the header after middleware, but never retain the
        // original shorter value that would truncate the inserted script/body.
        if let Some(length) = output.headers().get(header::CONTENT_LENGTH) {
            assert_eq!(
                length.to_str().unwrap().parse::<usize>().unwrap(),
                HTML.len() + SCRIPT.len()
            );
        }
        let bytes = to_bytes(output.into_body(), 4096).await.unwrap();
        let html = std::str::from_utf8(&bytes).unwrap();
        assert!(html.contains("<h1>/assets/flow-observations.js</h1>"));
        assert_eq!(html.matches(SCRIPT).count(), 1);
        assert_eq!(html, HTML.replace("</body>", &format!("{SCRIPT}</body>")));
    }

    #[tokio::test]
    async fn csp_directives_stay_separate_with_or_without_a_trailing_semicolon() {
        for policy in [
            "default-src 'none'; script-src 'none'; frame-ancestors 'none'",
            "default-src 'none'; script-src 'none'; frame-ancestors 'none';",
            "default-src 'none'; script-src 'none'; frame-ancestors 'none'; connect-src 'none'",
            "default-src 'none'; script-src 'self'; frame-ancestors 'none'; connect-src 'self';",
        ] {
            let mut input = response(Body::from(HTML));
            input.headers_mut().insert(
                "content-security-policy",
                HeaderValue::from_str(policy).unwrap(),
            );
            let output = apply(input).await;
            let actual = output.headers()["content-security-policy"]
                .to_str()
                .unwrap();
            let directives: Vec<_> = actual
                .split(';')
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .collect();
            assert_eq!(
                directives,
                [
                    "default-src 'none'",
                    "script-src 'self'",
                    "frame-ancestors 'none'",
                    "connect-src 'self'",
                ],
                "{policy}"
            );
        }
    }

    #[tokio::test]
    async fn untransformed_html_remains_exact_and_cannot_cache_per_response_proofs() {
        let large = HTML.replace("Book", &"x".repeat(2 * 1024 * 1024));
        let fragment = HTML.replace("</body>", "");
        let invalid_utf8 = vec![0xff, 0xfe];
        for bytes in [large.into_bytes(), fragment.into_bytes(), invalid_utf8] {
            let output = apply(response(Body::from(bytes.clone()))).await;
            isolated(&output);
            let actual = to_bytes(output.into_body(), 3 * 1024 * 1024).await.unwrap();
            assert_eq!(actual.as_ref(), bytes, "fallback must not distort HTML");
        }
        let streaming = Body::from_stream(Body::from(HTML).into_data_stream());
        assert!(streaming.size_hint().exact().is_none());
        let output = apply(response(streaming)).await;
        isolated(&output);
        assert_eq!(
            to_bytes(output.into_body(), 4096).await.unwrap().as_ref(),
            HTML.as_bytes()
        );
    }

    #[tokio::test]
    async fn public_script_asset_keeps_its_cache_policy() {
        let output = apply(script().await).await;
        assert_eq!(output.status(), StatusCode::OK);
        assert_eq!(
            output.headers()[header::CACHE_CONTROL],
            "public, max-age=300"
        );
    }
}
