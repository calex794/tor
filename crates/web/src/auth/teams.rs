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
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::{
    repositories::{
        translation_team_invites::PgTeamInvites, translation_teams::PgTranslationTeams,
    },
    services::translation_teams::{TeamError, TeamService, TranslationTeam},
};

#[derive(Clone)]
struct TeamState {
    accounts: PgAccounts,
    teams: TeamService,
    invites: PgTeamInvites,
    cookies: CookiePolicy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamForm {
    name: String,
    description: Option<String>,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteForm {
    recipient_email: String,
    csrf_token: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionForm {
    csrf_token: Option<String>,
}

pub(super) fn router(accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    let state = TeamState {
        teams: TeamService::new(PgTranslationTeams::new(accounts.pool().clone())),
        invites: PgTeamInvites::new(accounts.pool().clone()),
        accounts,
        cookies,
    };
    Router::new()
        .route("/me/teams", get(index).post(create))
        .route("/me/teams/{id}", get(detail))
        .route("/me/teams/{id}/invites", post(invite))
        .route("/me/teams/invites/{token}", get(invite_page))
        .route("/me/teams/invites/{token}/accept", post(accept))
        .route("/me/teams/invites/{token}/decline", post(decline))
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
fn team_error(error: TeamError) -> Response {
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
fn detail_html(team: &TranslationTeam, csrf: &str) -> String {
    format!(
        "<section class=\"section\"><div class=\"container\"><h1>{}</h1>\
         <p>{}</p><p>Вы владелец команды. Членство не даёт прав на книги.</p>\
         <h2>Пригласить участника</h2>\
         <form method=\"post\" action=\"/me/teams/{}/invites\">\
         <label>Электронная почта<input name=\"recipient_email\" type=\"email\" required maxlength=\"320\"></label>\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
         <button class=\"button\" type=\"submit\">Создать приглашение</button></form>\
         <p>Ссылка появляется один раз. Передайте её приглашённому самостоятельно.</p>\
         <p><a href=\"/me/teams\">Все команды</a></p></div></section>",
        escape_html(&team.name),
        escape_html(&team.description),
        team.id,
        escape_html(csrf)
    )
}

async fn index(State(s): State<TeamState>, headers: HeaderMap) -> Response {
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(status) => return session_failure(status),
    };
    let owner = match s.teams.for_owner(actor) {
        Ok(owner) => owner,
        Err(error) => return team_error(error),
    };
    let teams = match owner.list().await {
        Ok(teams) => teams,
        Err(error) => return team_error(error),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    let joined = match s.invites.joined(actor).await {
        Ok(teams) => teams,
        Err(error) => return team_error(error),
    };
    let mut html = list_html(&teams, &csrf);
    let mut fragment = String::from("<h2>Команды, в которых я участвую</h2><ul>");
    for (id, name) in joined {
        write!(
            fragment,
            "<li><a href=\"/me/teams/{}\">{}</a></li>",
            id,
            escape_html(&name)
        )
        .expect("HTML");
    }
    fragment.push_str("</ul>");
    if let Some(index) = html.rfind("</div></section>") {
        html.insert_str(index, &fragment);
    }
    page("Мои команды", &html, &cookies)
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
    let owner = match s.teams.for_owner(actor) {
        Ok(owner) => owner,
        Err(error) => return team_error(error),
    };
    match owner
        .create(&form.name, form.description.as_deref().unwrap_or(""))
        .await
    {
        Ok(team) => private_response(Redirect::to(&format!("/me/teams/{}", team.id)), &[]),
        Err(error) => team_error(error),
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
    let owner = match s.teams.for_owner(actor) {
        Ok(owner) => owner,
        Err(error) => return team_error(error),
    };
    match owner.get(team_id).await {
        Ok(team) => {
            let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
            page("Команда", &detail_html(&team, &csrf), &cookies)
        }
        Err(TeamError::Missing) => match s.invites.member_team(actor, team_id).await {
            Ok(name) => page(
                "Команда",
                &format!(
                    "<section class=\"section\"><div class=\"container\"><h1>{}</h1>\
                 <p>Вы участник команды. Права на книги назначаются отдельно.</p>\
                 <a href=\"/me/teams\">Все команды</a></div></section>",
                    escape_html(&name)
                ),
                &[],
            ),
            Err(error) => team_error(error),
        },
        Err(error) => team_error(error),
    }
}

/// Owner can share this one-time URL out of band; do not put bearer tokens in a DB or logs.
async fn invite(
    State(s): State<TeamState>,
    Path(team_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<InviteForm>,
) -> Response {
    if !s.cookies.same_origin(&headers)
        || !s.cookies.valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let owner = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(status) => return session_failure(status),
    };
    let token = generate_session_token();
    if let Err(error) = s
        .invites
        .invite(owner, team_id, &form.recipient_email, &token)
        .await
    {
        return team_error(error);
    }
    let link = format!("/me/teams/invites/{token}");
    page(
        "Приглашение",
        &format!(
            "<section class=\"section\"><div class=\"container\"><h1>Приглашение создано</h1>\
         <p>Передайте ссылку адресату. Она действует семь дней и не даёт прав на книги.</p>\
         <p><a href=\"{}\">Ссылка приглашения</a></p>\
         <p><a href=\"/me/teams/{}\">Вернуться к команде</a></p></div></section>",
            escape_html(&link),
            team_id
        ),
        &[],
    )
}

async fn invite_page(
    State(s): State<TeamState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    let recipient = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(status) => return session_failure(status),
    };
    let invite = match s.invites.detail(recipient, &token).await {
        Ok(value) => value,
        Err(error) => return team_error(error),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    let content = if invite.state == "pending" {
        format!(
            "<form method=\"post\" action=\"/me/teams/invites/{}/accept\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
             <button type=\"submit\">Принять</button></form>\
             <form method=\"post\" action=\"/me/teams/invites/{}/decline\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
             <button type=\"submit\">Отклонить</button></form>",
            token,
            escape_html(&csrf),
            token,
            escape_html(&csrf)
        )
    } else {
        format!("<p>Приглашение: {}</p>", escape_html(&invite.state))
    };
    page("Приглашение в команду", &format!(
        "<section class=\"section\"><div class=\"container\">\
         <h1>Приглашение в команду {}</h1>{}</div></section>",
        escape_html(&invite.team_name), content
    ), &cookies)
}


async fn accept(
    State(s): State<TeamState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Form(form): Form<DecisionForm>,
) -> Response {
    if !s.cookies.same_origin(&headers)
        || !s.cookies.valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let recipient = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(status) => return session_failure(status),
    };
    match s.invites.decide(recipient, &token, true).await {
        Ok(_) => private_response(Redirect::to("/me/teams"), &[]),
        Err(error) => team_error(error),
    }
}

async fn decline(
    State(s): State<TeamState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Form(form): Form<DecisionForm>,
) -> Response {
    if !s.cookies.same_origin(&headers)
        || !s.cookies.valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let recipient = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(status) => return session_failure(status),
    };
    match s.invites.decide(recipient, &token, false).await {
        Ok(_) => private_response(Redirect::to("/me/teams"), &[]),
        Err(error) => team_error(error),
    }
}
