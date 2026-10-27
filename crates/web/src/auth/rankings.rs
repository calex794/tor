//! F-05: private, no-script pairwise choices. No client-selected user/book IDs.
//! This API deliberately has no "start ranking" route: C must first implement
//! durable verified chapter completion, not infer completion from bookmarks.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tor_reading::services::PublicReadingRepository;
use tor_recommendations::{
    repositories::{RankingCommand, RankingStoreError},
    services::ranking::Outcome,
};

#[derive(Serialize)]
struct QuestionView {
    comparison_id: String,
    state_version: u64,
    book_a_id: String,
    book_b_id: String,
    book_a_title: String,
    book_b_title: String,
}

#[derive(Serialize)]
struct RankingView {
    revision: u64,
    state: &'static str,
    question: Option<QuestionView>,
}

#[derive(Deserialize)]
pub(super) struct ChoiceForm {
    expected_revision: u64,
    expected_state_version: u64,
    comparison_id: String,
    outcome: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ResumeForm {
    expected_revision: u64,
    csrf_token: Option<String>,
}

#[derive(Serialize)]
struct MutationReceipt {
    revision: u64,
    status: &'static str,
}

fn status(error: RankingStoreError) -> StatusCode {
    match error {
        RankingStoreError::Invalid => StatusCode::BAD_REQUEST,
        RankingStoreError::Conflict | RankingStoreError::Stale | RankingStoreError::Exhausted => {
            StatusCode::CONFLICT
        }
        RankingStoreError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn parse_outcome(raw: &str) -> Option<Outcome> {
    match raw {
        "a_wins" => Some(Outcome::AWins),
        "b_wins" => Some(Outcome::BWins),
        "tie" => Some(Outcome::Tie),
        "skip" => Some(Outcome::Skip),
        "defer_a" => Some(Outcome::DeferA),
        "defer_b" => Some(Outcome::DeferB),
        _ => None,
    }
}

fn csrf_valid(state: &AuthState, headers: &HeaderMap, token: Option<&str>) -> bool {
    state.cookies.same_origin(headers)
        && token.is_some_and(|token| state.cookies.valid_csrf(headers, token))
}

fn csrf_header(headers: &HeaderMap) -> Option<&str> {
    headers.get("x-csrf-token")?.to_str().ok()
}

async fn view(state: &AuthState, user_id: i64) -> Result<RankingView, StatusCode> {
    let snapshot = state.rankings.snapshot(user_id).await.map_err(status)?;
    let Some(question) = snapshot
        .next_question()
        .map_err(|error| status(error.into()))?
    else {
        return Ok(RankingView {
            revision: snapshot.revision,
            state: if snapshot.placement.is_some() {
                "exhausted"
            } else {
                "awaiting_verified_completion"
            },
            question: None,
        });
    };
    // Only public, unblocked books are displayed. Two bounded lookups,
    // regardless of how many prior comparisons this reader has.
    let (left_id, right_id) = (
        u64::try_from(question.book_a_id).map_err(|_| StatusCode::CONFLICT)?,
        u64::try_from(question.book_b_id).map_err(|_| StatusCode::CONFLICT)?,
    );
    let left = state
        .reader
        .public_book(left_id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::CONFLICT)?;
    let right = state
        .reader
        .public_book(right_id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::CONFLICT)?;
    Ok(RankingView {
        revision: snapshot.revision,
        state: "choose",
        question: Some(QuestionView {
            comparison_id: question.comparison_id,
            state_version: question.state_version,
            book_a_id: left.id.to_string(),
            book_b_id: right.id.to_string(),
            book_a_title: left.title,
            book_b_title: right.title,
        }),
    })
}

async fn actor(state: &AuthState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    match session_user(state, headers).await? {
        Some(user) => Ok(user),
        None => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn change_choice(
    state: &AuthState,
    headers: &HeaderMap,
    form: ChoiceForm,
    csrf: Option<&str>,
) -> Result<MutationReceipt, StatusCode> {
    let user = actor(state, headers).await?;
    if !csrf_valid(state, headers, csrf) {
        return Err(StatusCode::FORBIDDEN);
    }
    let outcome = parse_outcome(&form.outcome).ok_or(StatusCode::BAD_REQUEST)?;
    if form.comparison_id.len() > 100 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let result = state
        .rankings
        .apply(
            user,
            form.expected_revision,
            RankingCommand::Answer {
                expected_state_version: form.expected_state_version,
                comparison_id: form.comparison_id,
                outcome,
            },
        )
        .await
        .map_err(status)?;
    Ok(MutationReceipt {
        revision: result.snapshot.revision,
        status: "saved",
    })
}

async fn resume(
    state: &AuthState,
    headers: &HeaderMap,
    form: ResumeForm,
    csrf: Option<&str>,
) -> Result<MutationReceipt, StatusCode> {
    let user = actor(state, headers).await?;
    if !csrf_valid(state, headers, csrf) {
        return Err(StatusCode::FORBIDDEN);
    }
    let result = state
        .rankings
        .apply(
            user,
            form.expected_revision,
            RankingCommand::ResumeExhausted,
        )
        .await
        .map_err(status)?;
    Ok(MutationReceipt {
        revision: result.snapshot.revision,
        status: "resumed",
    })
}

pub(super) async fn api_state(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let user = match actor(&state, &headers).await {
        Ok(user) => user,
        Err(error) => return deny(error),
    };
    match view(&state, user).await {
        Ok(result) => private_response(Json(result), &[]),
        Err(error) => deny(error),
    }
}

pub(super) async fn api_choice(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(form): Json<ChoiceForm>,
) -> Response {
    let token = csrf_header(&headers);
    match change_choice(&state, &headers, form, token).await {
        Ok(result) => private_response(Json(result), &[]),
        Err(error) => deny(error),
    }
}

pub(super) async fn api_resume(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(form): Json<ResumeForm>,
) -> Response {
    let token = csrf_header(&headers);
    match resume(&state, &headers, form, token).await {
        Ok(result) => private_response(Json(result), &[]),
        Err(error) => deny(error),
    }
}

pub(super) async fn browser_choice(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<ChoiceForm>,
) -> Response {
    let csrf = form.csrf_token.clone();
    match change_choice(&state, &headers, form, csrf.as_deref()).await {
        Ok(_) => private_response(Redirect::to("/me/rankings"), &[]),
        Err(error) => deny(error),
    }
}

pub(super) async fn browser_resume(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<ResumeForm>,
) -> Response {
    let csrf = form.csrf_token.clone();
    match resume(&state, &headers, form, csrf.as_deref()).await {
        Ok(_) => private_response(Redirect::to("/me/rankings"), &[]),
        Err(error) => deny(error),
    }
}

pub(super) async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let user = match actor(&state, &headers).await {
        Ok(user) => user,
        Err(StatusCode::UNAUTHORIZED) => return private_response(Redirect::to("/login"), &[]),
        Err(error) => return deny(error),
    };
    let result = match view(&state, user).await {
        Ok(result) => result,
        Err(error) => return deny(error),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let content = match result.question {
        Some(question) => format!(
            r#"<p class="ranking-info">Выберите книгу, которая понравилась больше. Ваш рейтинг виден только вам.</p>
<div class="ranking-pair">
  <article class="ranking-pair__book"><p class="ranking-pair__label">Первая книга</p><h2 class="ranking-pair__title"><a href="/books/{}">{}</a></h2></article>
  <article class="ranking-pair__book"><p class="ranking-pair__label">Вторая книга</p><h2 class="ranking-pair__title"><a href="/books/{}">{}</a></h2></article>
</div>
<form method="post" action="/me/rankings/choice" class="ranking-actions">
  <input type="hidden" name="csrf_token" value="{}">
  <input type="hidden" name="expected_revision" value="{}">
  <input type="hidden" name="expected_state_version" value="{}">
  <input type="hidden" name="comparison_id" value="{}">
  <button class="ui-button ui-button--primary" type="submit" name="outcome" value="a_wins">Первая лучше</button>
  <button class="ui-button ui-button--primary" type="submit" name="outcome" value="b_wins">Вторая лучше</button>
  <button class="ui-button ui-button--ghost" type="submit" name="outcome" value="tie">Равнозначны</button>
  <button class="ui-button ui-button--ghost" type="submit" name="outcome" value="skip">Пропустить</button>
  <button class="ui-button ui-button--ghost" type="submit" name="outcome" value="defer_a">Отложить первую</button>
  <button class="ui-button ui-button--ghost" type="submit" name="outcome" value="defer_b">Отложить вторую</button>
</form>"#,
            question.book_a_id,
            pages::escape_html(&question.book_a_title),
            question.book_b_id,
            pages::escape_html(&question.book_b_title),
            pages::escape_html(&csrf),
            result.revision,
            question.state_version,
            pages::escape_html(&question.comparison_id),
        ),
        None if result.state == "exhausted" => format!(
            r#"<p class="ranking-info">Все предложенные пары пропущены. Можно возобновить сравнение.</p>
<form method="post" action="/me/rankings/resume">
  <input type="hidden" name="csrf_token" value="{}">
  <input type="hidden" name="expected_revision" value="{}">
  <button class="ui-button ui-button--primary" type="submit">Повторить вопросы</button>
</form>"#,
            pages::escape_html(&csrf),
            result.revision
        ),
        None => "<p class=\"ranking-info\">Пока нет книг для сравнения. Рейтинг появится после подтверждённого завершения чтения глав.</p>".into(),
    };
    let content = include_str!("../../templates/me_rankings.html")
        .replace("<!--RANKING_CONTENT-->", &content);
    private_response(
        pages::layout(
            "Мой рейтинг",
            "Личное сравнение прочитанных книг.",
            &content,
        ),
        &cookies,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_choice_values_are_allowed() {
        assert!(parse_outcome("a_wins").is_some());
        assert!(parse_outcome("b_wins").is_some());
        assert!(parse_outcome("tie").is_some());
        assert!(parse_outcome("skip").is_some());
        assert!(parse_outcome("defer_a").is_some());
        assert!(parse_outcome("defer_b").is_some());
        assert!(parse_outcome("claim_completed").is_none());
        assert!(parse_outcome("A_WINS").is_none());
    }
    #[test]
    fn ranking_markup_has_no_scripts_or_public_rec_fields() {
        let html = include_str!("../../templates/me_rankings.html");
        assert!(html.contains("<!--RANKING_CONTENT-->"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("Персональный анализ"));
    }
}
