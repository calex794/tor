//! Private administrative entry point. This is a URL alias, NOT an ACL.
use super::*;
use axum::{http::HeaderValue, response::Html};

const ADMIN_MENU: &str = r#"<section class="section"><div class="container">
<h1>Администрирование TOR</h1>
<p>Доступ разрешён только действующему администратору.</p>
<nav aria-label="Разделы администрирования">
<ul>
<li><a href="/admin/audience">Аудитория</a></li>
<li><a href="/admin/promotion/editorial">Редакционное продвижение</a></li>
<li><a href="/admin/recommendations">Рекомендации</a></li>
<li><a href="/admin/surprises">Сюрпризы</a></li>
<li><a href="/admin/mail">Почта</a></li>
<li><a href="/admin/support">Поддержка</a></li>
<li><a href="/admin/finance/refunds">Возвраты</a></li>
</ul>
</nav>
<p><a href="/me">Личный кабинет</a></p>
</div></section>"#;

fn body() -> Html<String> {
    pages::layout(
        "Администрирование",
        "Доступ к административным разделам TOR.",
        ADMIN_MENU,
    )
}

/// An unknown visitor and a logged-in non-admin get identical 404 responses.
/// Neither the alias nor the legacy /admin sections bypass the real role gate.
pub(super) async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let digest = session_token_digest(&session);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.administrator => (),
        Ok(_) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let mut response = private_response(body(), &[]);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::get, Router};
    use tower::ServiceExt;

    #[test]
    fn landing_contains_only_known_server_side_tools() {
        let html = body().0;
        assert!(html.contains("Администрирование TOR"));
        assert!(html.contains(r#"href="/admin/audience""#));
        assert!(html.contains(r#"href="/admin/support""#));
        assert!(html.contains(r#"href="/me""#));
        assert!(!html.contains("csrf_token"));
        assert!(!html.contains("security/traffic"));
    }

    #[tokio::test]
    async fn browser_percent_encoded_cyrillic_address_is_routable() {
        // HTTP transports a Cyrillic URL as a percent-encoded path.
        let router = Router::new().route(
            "/%D0%BE%D0%B4%D0%BC%D0%B5%D0%BD",
            get(|| async { StatusCode::NO_CONTENT }),
        );
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/%D0%BE%D0%B4%D0%BC%D0%B5%D0%BD")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
}
