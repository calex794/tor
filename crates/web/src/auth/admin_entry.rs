//! Private administrative entry point. This is a URL alias, NOT an ACL.
use super::*;
use axum::{http::HeaderValue, response::Html};

const ADMIN_MENU: &str = r#"<section class="section"><div class="container">
<h1>Администрирование TOR</h1>
<p>Доступ разрешён только действующему администратору.</p>
<nav aria-label="Разделы администрирования">
<ul>
<li><a href="/odmen/audience">Аудитория</a></li>
<li><a href="/odmen/promotion/editorial">Редакционное продвижение</a></li>
<li><a href="/odmen/recommendations">Рекомендации</a></li>
<li><a href="/odmen/surprises">Сюрпризы</a></li>
<li><a href="/odmen/mail">Почта</a></li>
<li><a href="/odmen/support">Поддержка</a></li>
<li><a href="/odmen/finance/refunds">Возвраты</a></li>
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
/// Every /odmen subroute retains its own authorization gate; the URL is not one.
pub(super) async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let digest = session_token_digest(&session);
    let verified = match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !verified.administrator {
        return deny(StatusCode::NOT_FOUND);
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
        assert!(html.contains(r#"href="/odmen/audience""#));
        assert!(html.contains(r#"href="/odmen/support""#));
        assert!(html.contains(r#"href="/me""#));
        assert!(!html.contains("csrf_token"));
        assert!(!html.contains("security/traffic"));
    }

    #[tokio::test]
    async fn ascii_admin_alias_is_routable() {
        // ASCII aliases survive proxy routing without percent-encoding.
        let router = Router::new().route("/odmen", get(|| async { StatusCode::NO_CONTENT }));
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/odmen")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
}
