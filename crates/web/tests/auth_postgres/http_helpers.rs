//! Shared HTTP test request helper to keep auth_postgres below the source cap.
use super::*;

pub(super) fn json_request(
    method: &str,
    uri: &str,
    body: &str,
    cookies: &str,
    origin: &str,
    csrf: &str,
) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .header("x-csrf-token", csrf)
        .body(Body::from(body.to_owned()))
        .expect("JSON HTTP");
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40001))));
    req
}

/// Author POST must really select existing canonical IDs, not forge an empty
/// selection and then expect genre-filtered discovery to return the book.
pub(super) async fn canonical_author_book_form(
    picker: &tor_books::repositories::postgres::PgAuthoring,
    command_key: &str,
    csrf: &str,
) -> String {
    async fn id(picker: &tor_books::repositories::postgres::PgAuthoring, q: &str) -> i64 {
        picker
            .suggest_taxonomies("genre", q, 15)
            .await
            .expect("curated genre lookup")
            .into_iter()
            .find(|term| term.label.eq_ignore_ascii_case(q))
            .unwrap_or_else(|| panic!("CI must seed approved genre {q}"))
            .id
    }
    let fantasy = id(picker, "fantasy").await;
    let adventure = id(picker, "adventure").await;
    format!(
        "title=%D0%9C%D0%BE%D1%8F%20%3Cscript%3E%20%D0%BA%D0%BD%D0%B8%D0%B3%D0%B0&target_language=ru&author_name=Alice+Voyager&genre_ids={fantasy}%2C{adventure}&command_key={command_key}&csrf_token={csrf}"
    )
}
