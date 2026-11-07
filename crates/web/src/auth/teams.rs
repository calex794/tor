//! S026: session-bound Team creation; Team is not a Book permission.
#![forbid(unsafe_code)]
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    pages::{escape_html, layout},
};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::repositories::translation_teams::{PgTranslationTeams, TeamError, TranslationTeam};

#[derive(Clone)]
struct TeamState {
    accounts: PgAccounts,
    teams: PgTranslationTeams,
    cookies: CookiePolicy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamForm {
    name: String,
    description: Option<String>,
    csrf_token: Option<String>,
}

pub(super) fn router(accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    let state = TeamState {
        teams: PgTranslationTeams::new(accounts.pool().clone()),
        accounts,
        cookies,
    };
    Router::new()
        .route("/me/teams", get(index).post(create))
        .route("/me/teams/{id}", get(detail))
        .layer(DefaultBodyLimit::max(8 * 1024))
        .with_state(state)
}

async fn actor(state: &TeamState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)
}

fn session_failure(status: StatusCode) -> Response {
    if status == StatusCode::UNAUTHORIZED {
        private_response(Redirect::to("/login"), &[])
    } else {
        private_response(status, &[])
    }
}
fn error(error: TeamError) -> Response {
    let status = match error {
        TeamError::Invalid => StatusCode::BAD_REQUEST,
        TeamError::Missing => StatusCode::NOT_FOUND,
        TeamError::Conflict => StatusCode::CONFLICT,
        TeamError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    private_response(status, &[])
}
fn page(title: &str, inner: &str, cookies: &[String]) -> Response {
    let mut response = private_response(layout(title, "Переводческие команды", inner), cookies);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow, noarchive"),
    );
    response
}
fn list_html(teams: &[TranslationTeam], csrf: &str) -> String {
    let mut html = String::from(
        "<section class=\"section\"><div class=\"container\"><h1>Мои команды</h1>\
         <p>Членство в команде не открывает чужие книги автоматически.</p><ul>",
    );
    for team in teams {
        write!(
            html,
            "<li><a href=\"/me/teams/{}\">{}</a></li>",
            team.id,
            escape_html(&team.name)
        )
        .expect("String formatting");
    }
    write!(
        html,
        "</ul><h2>Создать переводческую команду</h2>\
         <form method=\"post\" action=\"/me/teams\">\
         <label>Название<input name=\"name\" maxlength=\"100\" required></label>\
         <label>Описание<textarea name=\"description\" maxlength=\"2000\"></textarea></label>\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
         <button class=\"button\" type=\"submit\">Создать команду</button></form>\
         <p><a href=\"/me\">Личный кабинет</a></p></div></section>",
        escape_html(csrf)
    )
    .expect("String formatting");
    html
}
fn detail_html(team: &TranslationTeam) -> String {
    format!(
        "<section class=\"section\"><div class=\"container\"><h1>{}</h1>\
         <p>{}</p><p>Вы владелец команды. Приглашения и книги будут доступны \
         после отдельной проверки прав.</p>\
         <p><a href=\"/me/teams\">Все команды</a></p></div></section>",
        escape_html(&team.name),
        escape_html(&team.description)
    )
}

async fn index(State(s): State<TeamState>, headers: HeaderMap) -> Response {
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(status) => return session_failure(status),
    };
    let teams = match s.teams.list_owned(actor).await {
        Ok(teams) => teams,
        Err(error) => return error(error),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    page("Мои команды", &list_html(&teams, &csrf), &cookies)
}

async fn create(
    State(s): State<TeamState>,
    headers: HeaderMap,
    Form(form): Form<TeamForm>,
) -> Response {
    if !s.cookies.same_origin(&headers)
        || !s
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(status) => return session_failure(status),
    };
    match s
        .teams
        .create(actor, &form.name, form.description.as_deref().unwrap_or(""))
        .await
    {
        Ok(team) => private_response(Redirect::to(&format!("/me/teams/{}", team.id)), &[]),
        Err(error) => error(error),
    }
}

async fn detail(
    State(s): State<TeamState>,
    Path(team_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(status) => return session_failure(status),
    };
    match s.teams.owned(actor, team_id).await {
        Ok(team) => page("Команда", &detail_html(&team), &[]),
        Err(error) => error(error),
    }
}
