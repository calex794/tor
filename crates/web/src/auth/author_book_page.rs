//! Owner-only book management: keyset-paged chapters without giant pages.
use super::{deny, session_user, AuthState};
use crate::{
    auth_cookies::private_response,
    author_chapter_pages::{self, OwnerChapterPageOptions},
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_books::services::authoring::{AuthorError, OwnedChapterCursor};

#[derive(Deserialize, Default)]
pub(super) struct AuthorBookQuery {
    after: Option<String>,
}

/// Accept only positive decimal (chapter_number, chapter_id) keysets.
fn chapter_cursor(raw: Option<&str>) -> Result<Option<OwnedChapterCursor>, ()> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let (number, id) = raw.split_once(':').ok_or(())?;
    if number.is_empty()
        || id.is_empty()
        || !number.bytes().all(|value| value.is_ascii_digit())
        || !id.bytes().all(|value| value.is_ascii_digit())
    {
        return Err(());
    }
    let number: i32 = number.parse().map_err(|_| ())?;
    let id: i64 = id.parse().map_err(|_| ())?;
    if number <= 0 || id <= 0 {
        return Err(());
    }
    Ok(Some(OwnedChapterCursor { number, id }))
}

/// The owned-book guard precedes every chapter read, including malformed cursors.
pub(super) async fn page(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    Query(query): Query<AuthorBookQuery>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.authoring.owned_book(actor, id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let cursor = match chapter_cursor(query.after.as_deref()) {
        Ok(cursor) => cursor,
        Err(()) => return deny(StatusCode::BAD_REQUEST),
    };
    let published = match state.authoring.owned_book_is_public(actor, id).await {
        Ok(published) => published,
        Err(AuthorError::Missing) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let demand_count = match state.demand.current_count(id).await {
        Ok(count) => count,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let page = match state
        .authoring
        .list_owned_chapters_page(actor, id, cursor)
        .await
    {
        Ok(page) => page,
        Err(AuthorError::Missing) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let latest_cover = if state.cover_upload_enabled {
        match tor_books::repositories::cover_slots::PgCoverSlots::new(state.accounts.pool().clone())
            .owner_latest_status(id, actor)
            .await
        {
            Ok(latest) => latest,
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    } else {
        None
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        author_chapter_pages::detail_with_covers_with_demand_page(
            &book,
            published,
            &page.chapters,
            &csrf,
            OwnerChapterPageOptions {
                uploads_enabled: state.cover_upload_enabled,
                latest_job: latest_cover.as_deref(),
                demand_count,
                paid_editor_enabled: state.paid_author_available,
                next: page.next,
                has_previous_page: cursor.is_some(),
            },
        ),
        &cookies,
    )
}

/// The author sees the review result independently from any later
/// visibility restriction. No site reviewer metadata is loaded or rendered.
pub(super) async fn registration_status(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let (outcome, reason, visibility, held) =
        match state.authoring.owner_registration_status(actor, id).await {
            Ok(Some(review)) => review,
            Ok(None) => return deny(StatusCode::NOT_FOUND),
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        };
    private_response(
        axum::response::Html(render_registration_result(
            id,
            &outcome,
            reason.as_deref(),
            &visibility,
            held,
        )),
        &[],
    )
}

fn render_registration_result(
    book_id: i64,
    outcome: &str,
    reason: Option<&str>,
    visibility: &str,
    held: bool,
) -> String {
    let (label, explanation) = match outcome {
        "pending" => (
            "Ожидает модерации",
            "Заявка на регистрацию ещё не рассмотрена.",
        ),
        "approved" => (
            "Регистрация одобрена",
            "Модерация регистрации завершена. Публикацию выполняет владелец.",
        ),
        "rejected" => (
            "Регистрация отклонена",
            "Заявка на регистрацию отклонена модератором сайта.",
        ),
        "not_required" => (
            "Регистрация без проверки",
            "Для регистрации этой книги отдельное одобрение не требовалось.",
        ),
        _ => (
            "Статус регистрации не установлен",
            "Невозможно подтвердить результат регистрации.",
        ),
    };
    let visibility_note = match visibility {
        "active" => "Текущая видимость книги: разрешена.",
        "hidden" => "Текущая видимость книги: скрыта.",
        "blocked" => "Текущая видимость книги: заблокирована.",
        "pending_moderation" => "Текущая видимость книги: ожидает решения.",
        _ => "Текущая видимость книги: не установлена.",
    };
    let hold_note = if held {
        "<p>Дополнительно действует административное удержание.</p>"
    } else {
        ""
    };
    let reason_html = if outcome == "rejected" {
        reason
            .filter(|s| !s.trim().is_empty())
            .map_or_else(String::new, |reason| {
                format!(
                    "<section aria-label=\"Причина отказа\"><h2>Причина отказа</h2>\
                     <p>{}</p></section>",
                    crate::pages::escape_html(reason)
                )
            })
    } else {
        String::new()
    };
    crate::pages::layout(
        "Проверка регистрации книги",
        "Результат регистрации и отдельные ограничения видимости.",
        &format!(
            "<section class=\"section auth-section\"><div class=\"container\">\
             <h1>{label}</h1><p>{explanation}</p>\
             <p>{visibility_note}</p>{hold_note}{reason_html}\
             <p><a href=\"/me/books/{book_id}\">Вернуться к книге</a></p>\
             </div></section>"
        ),
    )
    .0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_result_exposes_rejection_reason_but_not_private_reviewer() {
        let html = render_registration_result(
            51,
            "rejected",
            Some("<script>reason</script>"),
            "hidden",
            false,
        );
        assert!(html.contains("Причина отказа"));
        assert!(html.contains("&lt;script&gt;reason&lt;/script&gt;"));
        assert!(!html.contains("<script>reason</script>"));
        assert!(!html.contains("reviewer_user_id"));
        assert!(!html.contains("moderator_user_id"));
        assert!(html.contains("/me/books/51"));
        let pending = render_registration_result(51, "pending", None, "pending_moderation", false);
        assert!(pending.contains("Ожидает модерации"));
        assert!(!pending.contains("Причина отказа</h2>"));
        let hidden_approved = render_registration_result(51, "approved", None, "hidden", true);
        assert!(hidden_approved.contains("Регистрация одобрена"));
        assert!(hidden_approved.contains("Текущая видимость книги: скрыта"));
        assert!(hidden_approved.contains("административное удержание"));
        assert!(!hidden_approved.contains("Регистрация отклонена"));
        assert!(!hidden_approved.contains("Причина отказа</h2>"));
    }

    #[test]
    fn b_12_author_cursor_never_accepts_unbounded_or_malformed_input() {
        assert_eq!(chapter_cursor(None), Ok(None));
        assert_eq!(
            chapter_cursor(Some("501:900")),
            Ok(Some(OwnedChapterCursor {
                number: 501,
                id: 900
            }))
        );
        for raw in [
            "",
            "0:10",
            "10:0",
            "-1:10",
            "1:-2",
            "1",
            "1:2:3",
            "1:abc",
            "+1:2",
            "1:2%22",
            "2147483648:1",
        ] {
            assert_eq!(chapter_cursor(Some(raw)), Err(()), "{raw}");
        }
    }
}
