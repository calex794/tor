//! Read-only staff preview for the 50/40/10 free-chapter shelf.
//! No viewable proofs or audience collection can be minted by a preview.
use super::{actor, pages, Runtime};
use crate::{auth_cookies::deny, pages as html};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::fmt::Write;
use std::time::Duration;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewQuery {
    book: Option<i64>,
    chapter: Option<i64>,
}

pub(crate) async fn page(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Query(query): Query<PreviewQuery>,
) -> Response {
    if let Err(code) = actor(&state, &headers).await {
        return pages::secure_page(deny(code), &[]);
    }
    let mut out = String::from(
        r#"<section class="section"><div class="container"><h1>Полка после бесплатной главы</h1><p>Правила: другие книги владельца 50%, книги других участников привязанной команды 40%, остальные книги сайта 10%. При отсутствии команды: 70% / 30%. Пропорции распределяются между выдачами, а не в одной полке из пяти карточек.</p><p>Предпросмотр использует анонимный basic-доступ. Он не выдаёт персональную информацию и не записывает показы.</p><form method="get" action="/me/recommendations/shelf-preview" class="author-form"><label>Исходная книга (ID)<input type="number" name="book" min="1" required></label><label>ID бесплатной главы для ротации<input type="number" name="chapter" min="1" required></label><button type="submit" class="button">Показать полку</button></form>"#,
    );
    if !state.enabled {
        out.push_str(r#"<p role="status">Рекомендательный блок сейчас отключён флагом TOR_RECOMMENDATIONS_ENABLED.</p>"#);
    }
    match (query.book, query.chapter) {
        (None, None) => {}
        (Some(book), Some(chapter)) if book > 0 && chapter > 0 => {
            let result = tokio::time::timeout(
                Duration::from_millis(950),
                state.store.reader_end_shelf(None, book, chapter),
            )
            .await;
            match result {
                Ok(Ok(feed)) => {
                    let counts = ["author", "teammate", "site"].map(|source| {
                        feed.items.iter().filter(|card| card.source == source).count()
                    });
                    let _ = write!(
                        out,
                        "<p data-testid=\"shelf-preview-counts\">Автор: {}, команда: {}, сайт: {}. Всего: {}.</p>",
                        counts[0],
                        counts[1],
                        counts[2],
                        feed.items.len()
                    );
                    if feed.items.is_empty() {
                        out.push_str("<p>Подходящих публичных книг не найдено.</p>");
                    } else {
                        out.push_str(&pages::shelf_panel_observed(&feed, None));
                    }
                }
                Ok(Err(_)) | Err(_) => out.push_str(
                    r#"<p role="alert">Предпросмотр сейчас недоступен. Проверьте исходную книгу и состояние PostgreSQL.</p>"#,
                ),
            }
        }
        _ => return pages::secure_page(deny(StatusCode::BAD_REQUEST), &[]),
    }
    out.push_str(
        r#"<p><a href="/me/recommendations/manage">Настройки рекомендаций</a></p></div></section>"#,
    );
    if let Err(code) = actor(&state, &headers).await {
        return pages::secure_page(deny(code), &[]);
    }
    pages::secure_page(
        html::layout(
            "Полка после главы",
            "Проверка источников рекомендаций",
            &out,
        ),
        &[],
    )
}
