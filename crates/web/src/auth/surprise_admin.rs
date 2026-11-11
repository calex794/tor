//! F-SURPRISE administrative forms; independently audited in F PostgreSQL.
use super::AuthState;
use crate::{
    auth_cookies::{deny, private_response},
    pages,
};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::session_token_digest;
use tor_recommendations::repositories::surprises::{
    admin::NewSurprise, PgSurprises, SurpriseError,
};

#[path = "surprise_experiment_view.rs"]
mod experiment_view;

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/admin/surprises", get(page))
        .route("/admin/surprises/policy", post(policy))
        .route("/admin/surprises/create", post(create))
        .route("/admin/surprises/{id}/state", post(set_state))
}

async fn administrator(state: &AuthState, headers: &HeaderMap) -> Result<[u8; 32], StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    let actor = state
        .accounts
        .session_actor(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !actor.administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(digest)
}

fn problem(error: SurpriseError) -> Response {
    deny(match error {
        SurpriseError::Forbidden => StatusCode::FORBIDDEN,
        SurpriseError::Invalid => StatusCode::BAD_REQUEST,
        SurpriseError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyForm {
    csrf_token: String,
    enabled: Option<String>,
    probability_per_mille: i16,
    experiment_enabled: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewForm {
    csrf_token: String,
    kind: String,
    title: String,
    body: String,
    book_id: String,
    editorial: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateForm {
    csrf_token: String,
    state: String,
}

async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let digest = match administrator(&state, &headers).await {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    let repo = PgSurprises::new(state.accounts.pool().clone());
    let (enabled, probability, experiment_enabled, items) = match repo.admin_overview(&digest).await
    {
        Ok(value) => value,
        Err(error) => return problem(error),
    };
    let summary = match repo.admin_experiment_summary(&digest).await {
        Ok(value) => value,
        Err(error) => return problem(error),
    };
    let reading = repo.admin_reading_experiment(&digest).await;
    if matches!(reading, Err(SurpriseError::Forbidden)) {
        return deny(StatusCode::FORBIDDEN);
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let csrf = pages::escape_html(&csrf);
    let mut body = format!(
        "<section class=\"section surprise-admin\"><div class=\"container\"><h1>Случайные открытия</h1>\
         <p>Бесплатные редакционные сюрпризы; оплаченные показы и бонусные деньги не поддерживаются.</p>\
         <p>Система также требует отдельный серверный переключатель TOR_SURPRISE_HTTP_ENABLED.</p>\
         <h2>Появления</h2><form method=\"post\" action=\"/admin/surprises/policy\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
         <label><input type=\"checkbox\" name=\"enabled\" value=\"on\" {}> Включить выбор сюрпризов</label>\
         <label>Вероятность, промилле <input type=\"number\" min=\"0\" max=\"1000\" name=\"probability_per_mille\" value=\"{probability}\"></label>\
         <label><input type=\"checkbox\" name=\"experiment_enabled\" value=\"on\" {}> Эксперимент 20% без подарков / 40% мягкий / 40% обычный (по умолчанию выключен)</label>\
         <p>Изменение вероятности или отключение работающего эксперимента останавливает запуск и выключает подарки. Повторное включение создаёт новый запуск; результаты не смешиваются.</p>\
         <button type=\"submit\">Сохранить настройки</button></form>\
         <h2>Новый сюрприз</h2><form method=\"post\" action=\"/admin/surprises/create\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
         <label>Тип <select name=\"kind\">\
         <option value=\"joke\">Шутка</option><option value=\"fact\">Факт</option>\
         <option value=\"quote\">Цитата</option><option value=\"book\">Книга</option></select></label>\
         <label>Название <input name=\"title\" maxlength=\"120\" required></label>\
         <label>Текст <textarea name=\"body\" maxlength=\"600\" required></textarea></label>\
         <label>ID публичной книги (обязательно для цитаты и рекомендации) <input name=\"book_id\" type=\"number\" min=\"1\"></label>\
         <label><input type=\"checkbox\" name=\"editorial\" value=\"on\"> Редакционное продвижение</label>\
         <button type=\"submit\">Создать черновик</button></form><h2>Содержимое</h2><ul>",
        if enabled { "checked" } else { "" },
        if experiment_enabled { "checked" } else { "" },
    );
    for item in items {
        let title = pages::escape_html(&item.title);
        let state = pages::escape_html(&item.state);
        let kind = pages::escape_html(&item.kind);
        let book = item.book_id.map_or_else(|| "—".into(), |v| v.to_string());
        let _ = write!(
            body,
            "<li><b>{title}</b> ({kind}, {state}) — ID {} — книга {book} {}",
            item.id,
            if item.editorial {
                "· Редакция"
            } else {
                ""
            }
        );
        let next = match item.state.as_str() {
            "draft" => &["approved"][..],
            "approved" => &["paused", "archived"][..],
            "paused" => &["archived"][..],
            _ => &[][..],
        };
        for target in next {
            let _ = write!(
                body,
                "<form method=\"post\" action=\"/admin/surprises/{}/state\" style=\"display:inline\">\
                 <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
                 <input type=\"hidden\" name=\"state\" value=\"{target}\">\
                 <button type=\"submit\">{target}</button></form>",
                item.id
            );
        }
        body.push_str("</li>");
    }
    body.push_str("</ul><h2>Эксперимент — служебные итоги за 7 завершённых суток UTC</h2>");
    body.push_str("<p>Это решения и занятые слоты, не подтверждённые показы и не прирост чтения. Меньше 20 разных аккаунтов в группе — значения скрыты. Отдельный отчёт по чтению расположен ниже.</p>");
    if summary.is_empty() {
        body.push_str("<p>Недостаточно данных для безопасного агрегированного отчёта.</p>");
    } else {
        body.push_str("<table><thead><tr><th>Группа</th><th>Аккаунты</th><th>Дневные решения</th><th>План</th><th>Занятые слоты</th><th>Открытия</th></tr></thead><tbody>");
        for entry in summary {
            let _ = write!(
                body,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                pages::escape_html(&entry.arm),
                entry.unique_accounts,
                entry.evaluated_days,
                entry.planned,
                entry.reserved_slots,
                entry.opens,
            );
        }
        body.push_str("</tbody></table>");
    }
    match reading {
        Ok(report) => experiment_view::render(&mut body, report.as_ref()),
        Err(_) => body.push_str("<p role=\"alert\">Отчёт по чтению недоступен. Результат эксперимента не определён; управление отключением остаётся доступным.</p>"),
    }
    body.push_str("</div></section>");
    private_response(
        pages::layout("Случайные открытия", "Редакционная панель сюрпризов", &body),
        &cookies,
    )
}

async fn policy(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<PolicyForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let digest = match administrator(&state, &headers).await {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    if form.enabled.as_deref() == Some("on")
        && form.experiment_enabled.as_deref() == Some("on")
        && !super::surprise_http::enabled()
    {
        return deny(StatusCode::CONFLICT);
    }
    let repo = PgSurprises::new(state.accounts.pool().clone());
    match repo
        .admin_policy(
            &digest,
            form.enabled.as_deref() == Some("on"),
            form.probability_per_mille,
            form.experiment_enabled.as_deref() == Some("on"),
        )
        .await
    {
        Ok(()) => private_response(Redirect::to("/admin/surprises"), &[]),
        Err(error) => problem(error),
    }
}

async fn create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<NewForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let digest = match administrator(&state, &headers).await {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    let book_id = if form.book_id.trim().is_empty() {
        None
    } else {
        match form.book_id.trim().parse::<i64>() {
            Ok(id) if id > 0 => Some(id),
            _ => return deny(StatusCode::BAD_REQUEST),
        }
    };
    let repo = PgSurprises::new(state.accounts.pool().clone());
    let input = NewSurprise {
        kind: &form.kind,
        title: &form.title,
        body: &form.body,
        book_id,
        editorial: form.editorial.as_deref() == Some("on"),
    };
    match repo.admin_create(&digest, &input).await {
        Ok(_) => private_response(Redirect::to("/admin/surprises"), &[]),
        Err(error) => problem(error),
    }
}

async fn set_state(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<StateForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let digest = match administrator(&state, &headers).await {
        Ok(digest) => digest,
        Err(status) => return deny(status),
    };
    let repo = PgSurprises::new(state.accounts.pool().clone());
    match repo.admin_state(&digest, id, &form.state).await {
        Ok(true) => private_response(Redirect::to("/admin/surprises"), &[]),
        Ok(false) => deny(StatusCode::CONFLICT),
        Err(error) => problem(error),
    }
}
