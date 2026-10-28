//! Public book-facing 404/503 pages from the original NEwBOO error templates.
//! No book title, existence hint, identifiers, personal data or request input.
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

pub(super) fn book_error(status: StatusCode) -> Response {
    let (code, title, message) = match status {
        StatusCode::NOT_FOUND => (
            "404",
            "Страница не найдена",
            "Возможно, страница была удалена, скрыта или ссылка устарела.",
        ),
        StatusCode::SERVICE_UNAVAILABLE => (
            "503",
            "Не удалось загрузить книгу",
            "Один из сервисов сейчас временно не отвечает. Попробуйте обновить страницу позже.",
        ),
        _ => return super::auth_cookies::private_response(status, &[]),
    };
    let body = include_str!("../templates/book_error.html")
        .replace("<!--ERROR_CODE-->", code)
        .replace("<!--ERROR_HEADING-->", title)
        .replace("<!--ERROR_MESSAGE-->", message)
        .replace(
            "<!--RETRY_ACTION-->",
            if status == StatusCode::SERVICE_UNAVAILABLE {
                "<a class=\"not-found-card__button\" href=\"\">Попробовать ещё раз</a>"
            } else {
                ""
            },
        );
    let html = super::pages::layout(title, message, &body).0.replacen(
        "</head>",
        "<meta name=\"robots\" content=\"noindex, nofollow\">\n</head>",
        1,
    );
    let mut response = (status, axum::response::Html(html)).into_response();
    // Reuse the same strict no-store/CSP security policy as other denied reads.
    response = super::auth_cookies::private_response(response, &[]);
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_static("30"),
    );
    if status != StatusCode::SERVICE_UNAVAILABLE {
        response.headers_mut().remove(header::RETRY_AFTER);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h_11_book_error_404_is_a_real_neboo_page_and_never_discloses_book() {
        let response = book_error(StatusCode::NOT_FOUND);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!response.headers().contains_key(header::RETRY_AFTER));
        let body = response.into_body();
        let bytes = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(axum::body::to_bytes(body, 32_768))
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("lang=\"ru\""));
        assert!(html.contains("not-found-card"));
        assert!(html.contains("Страница не найдена"));
        assert!(html.contains("noindex, nofollow"));
        assert!(html.contains("href=\"/catalog\""));
        assert!(!html.contains("book_id"));
        assert!(!html.contains("href=\"\""));
    }

    #[test]
    fn h_11_service_error_is_uncacheable_and_retryable() {
        let response = book_error(StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "30");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
