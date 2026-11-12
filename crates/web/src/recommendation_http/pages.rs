//! Reader controls and accessible cards. Every choice has a server-issued CAS key.
use super::{api, Runtime};
use crate::{
    auth_cookies::{deny, private_response},
    pages as html,
};
use axum::{
    extract::{Form, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::generate_session_token;
use tor_recommendations::domain::feedback::{
    book_id, valid_token, Choice, Feed, FeedbackError, Surface,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConsentForm {
    #[serde(default)]
    enabled: String,
    expected_revision: i64,
    command_key: String,
    csrf_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChoiceForm {
    book_id: String,
    choice: Choice,
    expected_revision: i64,
    command_key: String,
    csrf_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageQuery {
    after: Option<i64>,
}

pub(super) fn secure_page(body: impl IntoResponse, cookies: &[String]) -> Response {
    let mut response = private_response(body, cookies);
    response.headers_mut().insert("content-security-policy",HeaderValue::from_static(
        "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"));
    response
}
fn choice_form(book: &str, revision: i64, hidden: bool, csrf: &str) -> String {
    let (choice, label) = if hidden {
        ("not_interested", "Не интересно")
    } else {
        ("clear", "Вернуть в рекомендации")
    };
    format!("<form method=\"post\" action=\"/me/recommendations/feedback\"><input type=\"hidden\" name=\"book_id\" value=\"{book}\"><input type=\"hidden\" name=\"choice\" value=\"{choice}\"><input type=\"hidden\" name=\"expected_revision\" value=\"{revision}\"><input type=\"hidden\" name=\"command_key\" value=\"{key}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\"><button class=\"ui-button ui-button--small\" type=\"submit\">{label}</button></form>",book=html::escape_html(book),key=generate_session_token(),csrf=html::escape_html(csrf))
}
pub(super) fn panel(feed: &Feed, csrf: &str) -> String {
    panel_observed(feed, csrf, None)
}
pub(super) fn panel_observed(
    feed: &Feed,
    csrf: &str,
    observer: Option<(
        &dyn tor_contracts::exposure::ObservationIssuer,
        tor_contracts::exposure::ExposureSurface,
    )>,
) -> String {
    if feed.items.is_empty() {
        return String::new();
    }
    let mut out=format!("<section class=\"section\" data-rec-panel data-rec-csrf=\"{}\"><div class=\"container\"><h2>Что почитать дальше</h2><p>Рекомендация не предоставляет доступ к платным главам.</p><div class=\"book-grid\">",html::escape_html(csrf));
    for item in &feed.items {
        // Reject malformed identifiers before building a URL, even for internal DTOs.
        let Ok(id) = book_id(&item.book_id) else {
            continue;
        };
        let explanation = match item.reason.as_str() {
            "similar_by_content" => "Похожие темы и жанры",
            "readers_also_read" => "Также читали после этой книги",
            "manually_related" => "Связанная книга",
            "new_release" => "Недавно добавлена",
            "trending" => "Набирает интерес",
            _ => "Из доступного каталога",
        };
        let observation = observer
            .map(|(issuer, surface)| {
                crate::flow_exposure::attributes(
                    Some(issuer),
                    tor_contracts::exposure::CardObservation {
                        book_id: id,
                        surface,
                        editorial: None,
                    },
                )
            })
            .unwrap_or_default();
        let _=write!(out,"<article class=\"book-card\" data-rec-card{observation}><div class=\"book-card__body\"><h3 class=\"book-card__title\"><a data-rec-link data-flow-link href=\"/books/{id}\">{title}</a></h3><p>{explanation}</p>",title=html::escape_html(&item.title));
        if let Some(revision) = item.feedback_revision {
            out.push_str(&choice_form(&item.book_id, revision, true, csrf));
        }
        out.push_str("</div></article>");
    }
    out.push_str("</div><p><a href=\"/me/recommendations\">Скрытые книги</a></p></div></section>");
    out
}

/// Cross-promotion of discoverable owner/team/site Books after free chapters.
/// Browser-visible observation uses the shared bounded FLOW collector, not a
/// reader history or a second paid placement.
pub(super) fn shelf_panel_observed(
    feed: &Feed,
    issuer: Option<&dyn tor_contracts::exposure::ObservationIssuer>,
) -> String {
    if feed.items.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        r#"<section class="section reader-end-shelf" data-rec-panel aria-label="Рекомендации после главы"><div class="container"><h2>Что почитать дальше</h2><div class="home-shelf__track" aria-label="Подборка других книг">"#,
    );
    for item in &feed.items {
        let Ok(id) = book_id(&item.book_id) else {
            continue;
        };
        let title = html::escape_html(&item.title);
        let source = html::escape_html(&item.source);
        let relation = match item.source.as_str() {
            "author" => "Ещё у автора",
            "teammate" => "Из команды автора",
            "site" => "Другие книги сайта",
            _ => "Рекомендуем",
        };
        let observation = crate::flow_exposure::attributes(
            issuer,
            tor_contracts::exposure::CardObservation {
                book_id: id,
                surface: tor_contracts::exposure::ExposureSurface::ReaderEnd,
                editorial: None,
            },
        );
        let _ = write!(
            out,
            r#"<article class="book-mini-card book-mini-card--shelf" data-rec-card data-rec-source="{source}"{observation}><a class="book-mini-card__link" data-flow-link href="/books/{id}" aria-label="{title}"><div class="book-mini-card__cover book-shimmer"><img src="/covers/{id}/card.webp" alt="" loading="lazy" decoding="async"></div><div class="book-mini-card__body"><h3 class="book-mini-card__title">{title}</h3><small class="book-mini-card__meta">{relation}</small></div></a></article>"#,
        );
    }
    out.push_str("</div></div></section>");
    out
}

pub(super) async fn page(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Response {
    let (user_id, digest) = match actor(&state, &headers).await {
        Ok(value) => value,
        Err(StatusCode::UNAUTHORIZED) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let hidden = match state
        .store
        .hidden_books(user_id, query.after.unwrap_or(0))
        .await
    {
        Ok(value) => value,
        Err(error) => return api::error(error),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut body = String::from(
        "<section class=\"section\"><div class=\"container\"><h1>Рекомендации</h1>\
         <p>«Не интересно» скрывает книгу только для вас на 30 дней. \
         Вернуть книгу можно в списке ниже. Это явный выбор в вашей учётной записи; \
         он не включает учёт посещений и чтения.</p>",
    );
    body.push_str("<h2>Скрытые книги</h2>");
    for book in &hidden {
        let _ = write!(
            body,
            "<article><a href=\"/books/{}\">{}</a>{}</article>",
            html::escape_html(&book.book_id),
            html::escape_html(&book.title),
            choice_form(&book.book_id, book.revision, false, &csrf)
        );
    }
    if hidden.is_empty() {
        body.push_str("<p>Скрытых доступных книг нет.</p>");
    }
    if hidden.len() == 20 {
        if let Some(last) = hidden.last() {
            let _ = write!(
                body,
                "<p><a href=\"/me/recommendations?after={}\">Следующая страница</a></p>",
                html::escape_html(&last.book_id)
            );
        }
    }
    body.push_str("<p><a href=\"/me\">В кабинет</a></p></div></section>");
    if state.enabled {
        if let Ok(feed) =
            api::load(&state, Some((user_id, digest)), Surface::Library, None, 10).await
        {
            body.push_str(&panel(&feed, &csrf));
        }
    }
    secure_page(
        html::layout("Рекомендации", "Скрытые книги и личный выбор", &body),
        &cookies,
    )
}
pub(super) async fn consent(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Form(mut form): Form<ConsentForm>,
) -> Response {
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    if let Err(status) = actor(&state, &headers).await {
        return deny(status);
    }
    if form.enabled.is_empty() {
        form.enabled = "0".to_owned();
    }
    if !matches!(form.enabled.as_str(), "1" | "0") {
        return deny(StatusCode::BAD_REQUEST);
    }
    if tor_recommendations::services::feedback::validate_revision(form.expected_revision).is_err()
        || !valid_token(&form.command_key)
    {
        return api::error(FeedbackError::Invalid);
    }
    api::retired()
}

pub(super) async fn feedback(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Form(form): Form<ChoiceForm>,
) -> Response {
    if let Err(status) = csrf(&state, &headers, &form.csrf_token) {
        return deny(status);
    }
    let (user_id, _) = match actor(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    let book = match book_id(&form.book_id) {
        Ok(value) => value,
        Err(error) => return api::error(error),
    };
    match state
        .store
        .set_feedback(
            user_id,
            book,
            form.choice,
            form.expected_revision,
            &form.command_key,
        )
        .await
    {
        Ok(_) => private_response(Redirect::to("/me/recommendations"), &[]),
        Err(error) => api::error(error),
    }
}
pub(super) async fn script() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=300"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        include_str!("../../assets/recommendations.js"),
    )
        .into_response()
}

async fn actor(state: &Runtime, headers: &HeaderMap) -> Result<(i64, [u8; 32]), StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = tor_accounts::services::security::session_token_digest(&token);
    let actor = state
        .accounts
        .active_session_user(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok((actor, digest))
}
fn csrf(state: &Runtime, headers: &HeaderMap, token: &str) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers, token) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_recommendations::domain::feedback::FeedItem;
    #[test]
    fn cards_escape_titles_and_keep_plain_navigation_without_js() {
        let feed = Feed {
            items: vec![FeedItem {
                book_id: "12".into(),
                title: "<script>unsafe</script>".into(),
                reason: "similar_by_content".into(),
                source: "content".into(),
                algorithm_version: "recommendations_v1",
                ticket: Some("x".repeat(43)),
                feedback_revision: Some(0),
            }],
            generation_id: None,
            next_cursor: None,
            personalized: true,
        };
        let result = panel(&feed, &"c".repeat(43));
        assert!(result.contains("href=\"/books/12\""));
        assert!(!result.contains("<script>unsafe"));
        assert!(result.contains("&lt;script&gt;"));
        assert!(!result.contains("user_id"));
        assert!(!result.contains("?ticket="));
        assert!(!result.contains("data-rec-ticket"));
        assert!(!result.contains("<script"));
    }

    struct ProofIssuer;
    impl tor_contracts::exposure::ObservationIssuer for ProofIssuer {
        fn issue(&self, card: tor_contracts::exposure::CardObservation) -> Option<String> {
            assert_eq!(
                card.surface,
                tor_contracts::exposure::ExposureSurface::ReaderEnd
            );
            assert!(card.editorial.is_none());
            Some("A".repeat(43))
        }
    }

    #[test]
    fn reader_shelf_is_escaped_and_only_live_delivery_mints_observation() {
        let feed = Feed {
            items: vec![FeedItem {
                book_id: "12".into(),
                title: "<script>reader</script>".into(),
                reason: "similar_by_content".into(),
                source: "teammate".into(),
                algorithm_version: "reader_end_shelf_504010_v1",
                ticket: None,
                feedback_revision: None,
            }],
            generation_id: None,
            next_cursor: None,
            personalized: false,
        };
        let preview = shelf_panel_observed(&feed, None);
        assert!(preview.contains("data-rec-source=\"teammate\""));
        assert!(preview.contains("href=\"/books/12\""));
        assert!(preview.contains("/covers/12/card.webp"));
        assert!(preview.contains("&lt;script&gt;reader&lt;/script&gt;"));
        assert!(!preview.contains("data-flow-proof"));
        assert!(!preview.contains("<script>reader"));
        let delivered = shelf_panel_observed(&feed, Some(&ProofIssuer));
        assert!(delivered.contains(&format!("data-flow-proof=\"{}\"", "A".repeat(43))));
        assert!(delivered.contains("data-flow-link"));
    }

    #[test]
    fn free_reader_shelf_stays_after_navigation_and_before_discussion() {
        let html = include_str!("../../templates/reader.html");
        let nav = html.rfind("<!--CHAPTER_NAVIGATION-->").unwrap();
        let shelf = html.find("<!--READER_END_SHELF-->").unwrap();
        let discussion = html.find("<!--FREE_CHAPTER_COMMENTS-->").unwrap();
        assert!(nav < shelf && shelf < discussion);
        assert!(html.contains("<!--EDITORIAL_PROMOTION-->"));
    }
}
