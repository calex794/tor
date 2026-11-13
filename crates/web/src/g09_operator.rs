//! G-09 administrative reconciliation console. Strictly diagnostic operations.
use crate::{
    auth_cookies::{deny, private_response, CookiePolicy},
    pages,
};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::{fmt::Write, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_storage::{
    repositories::{
        reconciliation::{InventoryError, PgPrivateInventory},
        reconciliation_body::PgBodyAudit,
        reconciliation_operator::PgG09Operator,
        reconciliation_reverse::PgRegisteredAudit,
        s3::S3PrivateStore,
    },
    services::reconciliation::{PrivateInventoryLister, PrivateScope},
};

#[derive(Clone)]
struct OperatorState {
    accounts: PgAccounts,
    cookies: CookiePolicy,
    store: S3PrivateStore,
    inflight: Arc<Semaphore>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Reverse,
    Body,
}
fn target(dir: &str, scope: &str) -> Option<(Direction, PrivateScope)> {
    let direction = match dir {
        "s3-to-pg" => Direction::Forward,
        "pg-to-s3" => Direction::Reverse,
        "get-sha" => Direction::Body,
        _ => return None,
    };
    let scope = match scope {
        "chapter" => PrivateScope::Chapter,
        "chapter_asset" => PrivateScope::ChapterAsset,
        _ => return None,
    };
    Some((direction, scope))
}
fn slug(dir: Direction) -> &'static str {
    match dir {
        Direction::Forward => "s3-to-pg",
        Direction::Reverse => "pg-to-s3",
        Direction::Body => "get-sha",
    }
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub fn router(accounts: PgAccounts, store: S3PrivateStore, secure: bool, origin: String) -> Router {
    let state = OperatorState {
        accounts,
        cookies: CookiePolicy { secure, origin },
        store,
        inflight: Arc::new(Semaphore::new(1)),
    };
    Router::new()
        .route("/admin/storage/g09", get(page))
        .route(
            "/admin/storage/g09/{direction}/{scope}/{action}",
            post(command).layer(DefaultBodyLimit::max(1024)),
        )
        .with_state(state)
}
async fn authorize(state: &OperatorState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    match state
        .accounts
        .session_actor(&session_token_digest(&token))
        .await
    {
        Ok(Some(actor)) if actor.administrator => Ok(()),
        Ok(Some(_)) => Err(StatusCode::FORBIDDEN),
        Ok(None) => Err(StatusCode::UNAUTHORIZED),
        Err(_) => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}
fn action_button(
    body: &mut String,
    direction: Direction,
    scope: PrivateScope,
    action: &str,
    csrf: &str,
) {
    write!(
        body,
        "<form method=\"post\" action=\"/admin/storage/g09/{}/{}/{}\">
        <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
        <button type=\"submit\">{}</button></form>",
        slug(direction),
        scope.kind(),
        action,
        escape(csrf),
        match action {
            "run" if direction == Direction::Body => "GET/SHA-256: проверить до 8 объектов",
            "run" => "Запустить / продолжить",
            "resume" => "Возобновить после ошибок",
            _ => "Новый цикл",
        }
    )
    .expect("static action form");
}
async fn page(State(state): State<OperatorState>, headers: HeaderMap) -> Response {
    if let Err(e) = authorize(&state, &headers).await {
        return deny(e);
    }
    let bucket = PrivateInventoryLister::private_bucket(&state.store);
    let pool = state.accounts.pool().clone();
    let op = PgG09Operator::new(pool.clone());
    let forward = PgPrivateInventory::new(pool.clone());
    let reverse = PgRegisteredAudit::new(pool.clone());
    let body_audit = PgBodyAudit::new(pool);
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut body = String::from(
        "<section class=\"me-page\"><div class=\"container\">
        <h1>G-09 — сверка приватного хранилища</h1>
        <p>Диагностика без удаления и изменения пользовательских прав.
        GET/SHA-256 проверяет только шифротекст: до 8 объектов и 32 МиБ на нажатие.
        Ошибка S3 не означает потерю файла.</p>",
    );
    for direction in [Direction::Forward, Direction::Reverse, Direction::Body] {
        for scope in [PrivateScope::Chapter, PrivateScope::ChapterAsset] {
            let reverse_direction = direction == Direction::Reverse;
            let checkpoint = match if direction == Direction::Body {
                op.body_checkpoint(bucket, scope).await
            } else {
                op.checkpoint(bucket, scope, reverse_direction).await
            } {
                Ok(value) => value,
                Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
            };
            let findings = match if direction == Direction::Body {
                op.body_findings(bucket, scope).await
            } else {
                op.findings(bucket, scope, reverse_direction).await
            } {
                Ok(value) => value,
                Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
            };
            let (description, completed, exhausted) =
                if direction == Direction::Body {
                    match body_audit.progress(bucket, scope).await {
                        Ok(Some(p)) => (
                            format!(
                            "Просмотрено записей: {}/{}; SHA совпал: {}; подтверждённые 404: {};
                             искажение байтов: {}; расхождение метаданных: {};
                             пропущено (pending/deleting/более 4 МиБ): {};
                             неопределённые попытки: {}; повторы: {}",
                            p.scanned, p.expected, p.verified, p.missing,
                            p.body_mismatch, p.metadata_mismatch,
                            p.skipped, p.unknown_attempts, p.retries
                        ),
                            p.completed,
                            p.exhausted,
                        ),
                        Ok(None) => ("Не запускался (только по кнопке)".to_owned(), false, false),
                        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
                    }
                } else if reverse_direction {
                    match reverse.progress(bucket, scope).await {
                        Ok(Some(p)) => (
                            format!(
                                "Ожидалось: {}; просмотрено: {}; со ссылками: {}; исправно: {};
                            отсутствуют: {}; из них со ссылками: {}; расхождения: {};
                            переходные: {}; неопределённые попытки: {}; повторы: {}",
                                p.expected,
                                p.scanned,
                                p.has_ref,
                                p.healthy,
                                p.missing,
                                p.referenced_missing,
                                p.mismatch,
                                p.recently_pending,
                                p.unknown_attempts,
                                p.retries
                            ),
                            p.completed,
                            p.exhausted,
                        ),
                        Ok(None) => ("Ещё не запускался".to_owned(), false, false),
                        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
                    }
                } else {
                    match forward.progress(bucket, scope).await {
                        Ok(Some(p)) => (
                            format!(
                                "Просмотрено: {}; в реестре: {}; переходные: {}; кандидаты: {};
                            расхождения размеров: {}; неизвестные объекты: {};
                            ошибки S3: {}; попытки: {}",
                                p.scanned,
                                p.registered,
                                p.recently_pending,
                                p.unregistered_candidates,
                                p.size_mismatch,
                                p.unknown,
                                p.unknown_attempts,
                                p.retries
                            ),
                            p.completed,
                            p.exhausted,
                        ),
                        Ok(None) => ("Ещё не запускался".to_owned(), false, false),
                        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
                    }
                };
            write!(body,
                "<section><h2>{} — {}</h2><p>{}</p><p>Цикл: {}; контрольная точка UTC: {}; аренда: {}</p>",
                match direction { Direction::Forward => "S3 → PostgreSQL", Direction::Reverse => "PostgreSQL → S3", Direction::Body => "GET/SHA-256 шифротекста" },
                if scope == PrivateScope::Chapter { "главы" } else { "изображения глав" },
                description,
                checkpoint.as_ref().map_or(0, |c| c.cycle),
                checkpoint.as_ref().and_then(|c| c.checkpoint.as_deref()).unwrap_or("нет"),
                if checkpoint.as_ref().is_some_and(|c| c.leased) { "активна" } else { "нет" }
            ).expect("static progress");
            if !completed && !exhausted {
                action_button(&mut body, direction, scope, "run", &csrf);
            }
            if exhausted {
                action_button(&mut body, direction, scope, "resume", &csrf);
            }
            if completed {
                action_button(&mut body, direction, scope, "next", &csrf);
            }
            body.push_str("<h3>Последние отклонения</h3><ul>");
            for f in findings {
                write!(
                    body,
                    "<li><code>{}</code>: {}; байт: {}; ссылка: {}; UTC: {}</li>",
                    escape(&f.fingerprint),
                    escape(&f.status),
                    f.size_bytes
                        .map_or_else(|| "неизвестно".to_owned(), |n| n.to_string()),
                    f.has_ref.map_or("неизвестно", |n| if n {
                        "есть"
                    } else {
                        "нет"
                    }),
                    escape(&f.observed)
                )
                .expect("sanitized anomaly");
            }
            body.push_str("</ul></section>");
        }
    }
    body.push_str("</div></section>");
    let mut response = private_response(
        pages::layout("G-09 — аудит хранения", "Операторская панель", &body),
        &cookies,
    );
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandForm {
    csrf_token: String,
}
async fn command(
    State(state): State<OperatorState>,
    Path((dir, kind, action)): Path<(String, String, String)>,
    headers: HeaderMap,
    Form(form): Form<CommandForm>,
) -> Response {
    if let Err(e) = authorize(&state, &headers).await {
        return deny(e);
    }
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some((direction, scope)) = target(&dir, &kind) else {
        return deny(StatusCode::BAD_REQUEST);
    };
    let bucket = PrivateInventoryLister::private_bucket(&state.store);
    let pool = state.accounts.pool().clone();
    let forward = PgPrivateInventory::new(pool.clone());
    let reverse = PgRegisteredAudit::new(pool.clone());
    let body_audit = PgBodyAudit::new(pool);
    let result = match (direction, action.as_str()) {
        (Direction::Forward, "next") => forward.begin_next_cycle(bucket, scope).await,
        (Direction::Reverse, "next") => reverse.begin_next_cycle(bucket, scope).await,
        (Direction::Body, "next") => body_audit.begin_next_cycle(bucket, scope).await,
        (Direction::Forward, "resume") => forward.resume_exhausted(bucket, scope).await,
        (Direction::Reverse, "resume") => reverse.resume_exhausted(bucket, scope).await,
        (Direction::Body, "resume") => body_audit.resume_exhausted(bucket, scope).await,
        (_, "run") => {
            let available = match direction {
                Direction::Forward => forward
                    .progress(bucket, scope)
                    .await
                    .map(|p| p.is_none_or(|p| !p.completed && !p.exhausted)),
                Direction::Reverse => reverse
                    .progress(bucket, scope)
                    .await
                    .map(|p| p.is_none_or(|p| !p.completed && !p.exhausted)),
                Direction::Body => body_audit
                    .progress(bucket, scope)
                    .await
                    .map(|p| p.is_none_or(|p| !p.completed && !p.exhausted)),
            };
            match available {
                Ok(true) => {}
                Ok(false) => return deny(StatusCode::CONFLICT),
                Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
            }
            let Ok(permit) = state.inflight.clone().try_acquire_owned() else {
                return deny(StatusCode::CONFLICT);
            };
            let store = state.store.clone();
            tokio::spawn(async move {
                let _permit = permit;
                for _ in 0..32 {
                    let outcome = match direction {
                        Direction::Forward => forward
                            .scan_page(&store, scope)
                            .await
                            .map(|p| p.map(|p| p.completed)),
                        Direction::Reverse => reverse
                            .scan_page(&store, scope)
                            .await
                            .map(|p| p.map(|p| p.completed || p.exhausted)),
                        Direction::Body => body_audit
                            .scan_page(&store, scope)
                            .await
                            .map(|p| p.map(|p| p.completed || p.exhausted)),
                    };
                    // Body scanning costs at most 8 * 4 MiB per explicit click;
                    // never inherit the 32-page LIST/HEAD loop budget.
                    if direction == Direction::Body {
                        break;
                    }
                    match outcome {
                        Ok(Some(false)) => tokio::time::sleep(Duration::from_millis(75)).await,
                        _ => break,
                    }
                }
            });
            return accepted();
        }
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    match result {
        Ok(true) => accepted(),
        Ok(false) | Err(InventoryError::Busy) => deny(StatusCode::CONFLICT),
        Err(InventoryError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(InventoryError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
fn accepted() -> Response {
    private_response(
        (
            StatusCode::ACCEPTED,
            axum::response::Html("<p>Принято. <a href=\"/admin/storage/g09\">К панели</a></p>"),
        ),
        &[],
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_private_directions_and_scopes() {
        assert!(target("pg-to-s3", "chapter").is_some());
        assert!(target("get-sha", "chapter").is_some());
        assert!(target("s3-to-pg", "chapter_asset").is_some());
        assert!(target("s3-to-pg", "cover").is_none());
        assert!(target("get-sha", "cover").is_none());
        assert!(target("delete", "chapter").is_none());
    }
    #[test]
    fn escaped_finding() {
        assert_eq!(escape("<script>"), "&lt;script&gt;");
    }
}
