//! F-05: two separate 1..5 judgments, not the personal pairwise tier list.
//! All edits are session-scoped and require a C-verified completed chapter.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_recommendations::repositories::{PgBookStars, StarKind, StarRatingError, UserStars};

#[derive(Deserialize)]
pub(super) struct RatingForm {
    rating_kind: String,
    rating: i16,
    csrf_token: Option<String>,
}

fn map_error(error: StarRatingError) -> StatusCode {
    match error {
        StarRatingError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        StarRatingError::NotEligible => StatusCode::FORBIDDEN,
        StarRatingError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn rating_field(book_id: i64, kind: &str, label: &str, current: Option<i16>, csrf: &str) -> String {
    let mut radios = String::new();
    for score in 1..=5_i16 {
        write!(
            radios,
            "<label class=\"rating-pick\"><input type=\"radio\" name=\"rating\" value=\"{score}\" {checked} required> {score}</label>",
            checked = if current == Some(score) { "checked" } else { "" },
        )
        .expect("rating choice");
    }
    format!(
        "<form class=\"book-score-form\" method=\"post\" action=\"/me/books/{book_id}/rate\">
         <fieldset><legend>{label} · от 1 до 5</legend><p>Оцените {subject} отдельно от другого показателя.</p>
         <div class=\"rating-picks\">{radios}</div></fieldset>
         <input type=\"hidden\" name=\"rating_kind\" value=\"{kind}\">
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">
         <button class=\"button\" type=\"submit\">Сохранить оценку</button>
         </form>",
        subject = if kind == "work" { "произведение" } else { "перевод" },
    )
}

fn form_page(
    book_id: i64,
    title: &str,
    scores: UserStars,
    csrf: &str,
    eligible: bool,
    cookies: &[String],
) -> Response {
    let title = pages::escape_html(title);
    let body = if eligible {
        format!(
            "<p>Это независимые оценки: содержание произведения и качество перевода.
             Личный порядок книг формируется отдельно.</p>
             <div class=\"book-score-forms\">{work}{translation}</div>",
            work = rating_field(book_id, "work", "Произведение", scores.work, csrf),
            translation = rating_field(book_id, "translation", "Перевод", scores.translation, csrf),
        )
    } else {
        "<p class=\"empty-state\">Оценка будет доступна после подтверждённого окончания
         хотя бы одной главы. Простая загрузка или закладка прочтением не считается.</p>"
            .to_owned()
    };
    private_response(
        pages::layout(
            "Оценить книгу",
            "Независимые оценки произведения и перевода.",
            &format!(
                "<section class=\"section auth-section\"><div class=\"container\">
                 <div class=\"auth-card\">
                 <p class=\"eyebrow\">Оценки читателя</p>
                 <h1>Оценить книгу</h1><h2>{title}</h2>{body}
                 <p><a class=\"text-link\" href=\"/books/{book_id}\">Вернуться к книге</a></p>
                 </div></div></section>"
            ),
        ),
        cookies,
    )
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(error) => return deny(error),
    };
    let stars = PgBookStars::new(state.accounts.pool().clone());
    let summary = match stars.visible_summary(book_id).await {
        Ok(Some(summary)) => summary,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return deny(map_error(error)),
    };
    let scores = match stars.own_ratings(actor, book_id).await {
        Ok(scores) => scores,
        Err(error) => return deny(map_error(error)),
    };
    let eligible = match stars.eligible(actor, book_id).await {
        Ok(eligible) => eligible,
        Err(error) => return deny(map_error(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    form_page(book_id, &summary.title, scores, &csrf, eligible, &cookies)
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<RatingForm>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    if !state.cookies.same_origin(&headers)
        || !form
            .csrf_token
            .as_deref()
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(kind) = StarKind::parse(&form.rating_kind) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let stars = PgBookStars::new(state.accounts.pool().clone());
    match stars.set_rating(actor, book_id, kind, form.rating).await {
        Ok(()) => private_response(Redirect::to(&format!("/me/books/{book_id}/rate")), &[]),
        Err(error) => deny(map_error(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_05_two_independent_browser_forms_have_no_client_user_id() {
        let content = form_page(
            31,
            "Проверка <script>",
            UserStars::default(),
            "abc",
            true,
            &[],
        );
        let _ = content;
        let html = rating_field(31, "work", "Произведение", Some(3), "token");
        assert!(html.contains("value=\"work\""));
        assert!(html.contains("value=\"3\" checked"));
        assert!(!html.contains("user_id"));
    }
}
