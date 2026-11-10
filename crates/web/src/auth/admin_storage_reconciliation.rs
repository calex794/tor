//! G-09/#748: gated, administrator-only operator console for private S3 audit.
//! Commands advance at most one checkpoint page; they cannot read bodies, GC,
//! delete chapters, change entitlements or accept arbitrary bucket/object keys.
use super::AuthState;
use axum::Router;

#[cfg(not(feature = "s3-paid"))]
pub(super) fn router() -> Router<AuthState> {
    Router::new()
}

#[cfg(feature = "s3-paid")]
mod enabled {
    use super::*;
    use crate::{
        auth_cookies::{deny, private_response},
        pages,
    };
    use axum::{
        extract::{rejection::FormRejection, DefaultBodyLimit, Form, State},
        http::{HeaderMap, HeaderValue, StatusCode},
        response::{IntoResponse, Redirect, Response},
        routing::{get, post},
    };
    use serde::Deserialize;
    use std::fmt::Write;
    use tor_accounts::services::security::session_token_digest;
    use tor_storage::{
        domain::provider::valid_private_bucket,
        repositories::{
            reconciliation::{InventoryError, InventoryProgress, PgPrivateInventory},
            reconciliation_operator::PgReconciliationCommands,
            reconciliation_reverse::{PgRegisteredAudit, RegisteredAuditProgress},
            s3::S3PrivateStore,
        },
        services::reconciliation::PrivateScope,
    };

    const PATH: &str = "/admin/storage/reconciliation";

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct CommandForm {
        csrf_token: String,
        direction: String,
        scope: String,
        action: String,
    }

    #[derive(Clone, Copy)]
    enum Direction {
        S3ToPg,
        PgToS3,
    }

    impl Direction {
        fn name(self) -> &'static str {
            match self {
                Self::S3ToPg => "s3_to_pg",
                Self::PgToS3 => "pg_to_s3",
            }
        }
        fn label(self) -> &'static str {
            match self {
                Self::S3ToPg => "S3 → PostgreSQL",
                Self::PgToS3 => "PostgreSQL → S3",
            }
        }
    }

    fn configured_bucket() -> Result<String, StatusCode> {
        let bucket =
            std::env::var("TOR_PAID_S3_BUCKET").map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if !valid_private_bucket(&bucket) {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        Ok(bucket)
    }

    fn enabled() -> bool {
        std::env::var("TOR_G09_OPERATOR_ENABLED").as_deref() == Ok("1")
    }

    fn scope(raw: &str) -> Option<PrivateScope> {
        match raw {
            "chapter" => Some(PrivateScope::Chapter),
            "chapter_asset" => Some(PrivateScope::ChapterAsset),
            _ => None,
        }
    }

    fn direction(raw: &str) -> Option<Direction> {
        match raw {
            "s3_to_pg" => Some(Direction::S3ToPg),
            "pg_to_s3" => Some(Direction::PgToS3),
            _ => None,
        }
    }

    fn code(error: InventoryError) -> StatusCode {
        match error {
            InventoryError::Invalid => StatusCode::BAD_REQUEST,
            InventoryError::Busy => StatusCode::CONFLICT,
            InventoryError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// Database-backed session verification on every GET and POST; a cookie
    /// alone and a self-reported role never authorize this console.
    async fn admin(state: &AuthState, headers: &HeaderMap) -> Result<i64, StatusCode> {
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
        Ok(actor.actor.user_id.get())
    }

    fn form(
        csrf: &str,
        direction: Direction,
        scope: PrivateScope,
        action: &str,
        label: &str,
    ) -> String {
        format!(
            r#"<form method="post" action="/admin/storage/reconciliation/run">
              <input type="hidden" name="csrf_token" value="{}">
              <input type="hidden" name="direction" value="{}">
              <input type="hidden" name="scope" value="{}">
              <input type="hidden" name="action" value="{action}">
              <button type="submit">{label}</button></form>"#,
            pages::escape_html(csrf),
            direction.name(),
            scope.kind(),
        )
    }

    fn forward_details(p: Option<&InventoryProgress>) -> String {
        match p {
            None => "Сканирование ещё не запускалось.".to_owned(),
            Some(s) => format!(
                "Проверено: {} · учтено: {} · недавно загружено: {} ·                  кандидатов без реестра: {} · несовпадение размера: {} ·                  неизвестно: {} · завершено: {}",
                s.scanned, s.registered, s.recently_pending,
                s.unregistered_candidates, s.size_mismatch,
                s.unknown, s.completed
            ),
        }
    }

    fn reverse_details(p: Option<&RegisteredAuditProgress>) -> String {
        match p {
            None => "Сканирование ещё не запускалось.".to_owned(),
            Some(s) => format!(
                "Цикл: {} · покрытие: {}/{} · с ссылками: {} ·                  существует: {} · отсутствует: {} · закреплённых отсутствует: {} ·                  несовпадение метаданных: {} · недавно pending: {} ·                  недоступные попытки: {} · повторы: {} · остановлено: {} · завершено: {}",
                s.cycle, s.scanned, s.expected, s.has_ref, s.healthy,
                s.missing, s.referenced_missing, s.mismatch, s.recently_pending,
                s.unknown_attempts, s.retries, s.exhausted, s.completed
            ),
        }
    }

    fn render_card(
        csrf: &str,
        dir: Direction,
        scope: PrivateScope,
        details: &str,
        completed: bool,
        exhausted: bool,
    ) -> String {
        let mut out = String::new();
        let family = if scope == PrivateScope::Chapter {
            "Зашифрованные редакции глав"
        } else {
            "Зашифрованные изображения глав"
        };
        write!(
            out,
            "<article class=\"me-now-card\"><h2>{} — {family}</h2><p>{details}</p>",
            dir.label()
        )
        .expect("static operator card");
        if completed {
            out.push_str(&form(csrf, dir, scope, "next_cycle", "Начать новый цикл"));
        } else if exhausted {
            if matches!(dir, Direction::PgToS3) {
                out.push_str(&form(
                    csrf,
                    dir,
                    scope,
                    "resume_exhausted",
                    "Разрешить повтор",
                ));
            }
        } else {
            out.push_str(&form(csrf, dir, scope, "step", "Обработать одну порцию"));
        }
        out.push_str("</article>");
        out
    }

    pub(super) async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
        if !enabled() {
            return deny(StatusCode::NOT_FOUND);
        }
        if let Err(status) = admin(&state, &headers).await {
            return deny(status);
        }
        let bucket = match configured_bucket() {
            Ok(bucket) => bucket,
            Err(status) => return deny(status),
        };
        let forward = PgPrivateInventory::new(state.accounts.pool().clone());
        let reverse = PgRegisteredAudit::new(state.accounts.pool().clone());
        let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
        let mut content = String::new();
        for scope in [PrivateScope::Chapter, PrivateScope::ChapterAsset] {
            let f = match forward.progress(&bucket, scope).await {
                Ok(result) => result,
                Err(error) => return deny(code(error)),
            };
            let r = match reverse.progress(&bucket, scope).await {
                Ok(result) => result,
                Err(error) => return deny(code(error)),
            };
            content.push_str(&render_card(
                &csrf,
                Direction::S3ToPg,
                scope,
                &forward_details(f.as_ref()),
                f.as_ref().is_some_and(|v| v.completed),
                false,
            ));
            content.push_str(&render_card(
                &csrf,
                Direction::PgToS3,
                scope,
                &reverse_details(r.as_ref()),
                r.as_ref().is_some_and(|v| v.completed),
                r.as_ref().is_some_and(|v| v.exhausted),
            ));
        }
        let html = pages::layout(
            "Сверка приватного хранилища — администратор",
            "Только безопасная инвентаризация без удаления.",
            &format!(
                r#"<section class="section"><div class="container">
                  <h1>Сверка приватного хранилища</h1>
                  <p>Только две зашифрованные области chapter и chapter_asset.
                     Одна операция — не более одной S3 LIST страницы (1000 объектов)
                     или 128 HEAD проверок. Не удаляет данные, редакции или покупки.
                     Подтверждение SHA в HEAD проверяет метаданные, а не байты файла.</p>
                  {content}<p><a href="/me">К кабинету</a></p></div></section>"#,
            ),
        );
        let mut response = private_response(html, &cookies).into_response();
        response.headers_mut().insert(
            "x-robots-tag",
            HeaderValue::from_static("noindex, nofollow"),
        );
        response
    }

    pub(super) async fn run(
        State(state): State<AuthState>,
        headers: HeaderMap,
        submitted: Result<Form<CommandForm>, FormRejection>,
    ) -> Response {
        if !enabled() {
            return deny(StatusCode::NOT_FOUND);
        }
        let actor = match admin(&state, &headers).await {
            Ok(id) => id,
            Err(status) => return deny(status),
        };
        let Form(input) = match submitted {
            Ok(form) => form,
            Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        };
        if !state.cookies.same_origin(&headers)
            || !state.cookies.valid_csrf(&headers, &input.csrf_token)
        {
            return deny(StatusCode::FORBIDDEN);
        }
        let (Some(direction), Some(scope)) = (direction(&input.direction), scope(&input.scope))
        else {
            return deny(StatusCode::BAD_REQUEST);
        };
        if !matches!(
            input.action.as_str(),
            "step" | "next_cycle" | "resume_exhausted"
        ) || (input.action == "resume_exhausted" && !matches!(direction, Direction::PgToS3))
        {
            return deny(StatusCode::BAD_REQUEST);
        }
        let bucket = match configured_bucket() {
            Ok(bucket) => bucket,
            Err(status) => return deny(status),
        };
        let pool = state.accounts.pool().clone();
        let audit = PgReconciliationCommands::new(pool.clone());
        if let Err(error) = audit
            .record(
                actor,
                direction.name(),
                scope,
                match input.action.as_str() {
                    "step" => "step",
                    "next_cycle" => "next_cycle",
                    _ => "resume_exhausted",
                },
            )
            .await
        {
            return deny(code(error));
        }
        let outcome = if input.action == "next_cycle" {
            match direction {
                Direction::S3ToPg => {
                    PgPrivateInventory::new(pool)
                        .begin_next_cycle(&bucket, scope)
                        .await
                }
                Direction::PgToS3 => {
                    PgRegisteredAudit::new(pool)
                        .begin_next_cycle(&bucket, scope)
                        .await
                }
            }
        } else if input.action == "resume_exhausted" {
            PgRegisteredAudit::new(pool)
                .resume_exhausted(&bucket, scope)
                .await
        } else {
            // Construct provider ONLY after session, CSRF, scope and audit gates.
            // No request controls a bucket, object key, endpoint or credential.
            let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
            let store = match S3PrivateStore::new(aws_sdk_s3::Client::new(&aws), bucket) {
                Ok(s3) => s3,
                Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
            };
            match direction {
                Direction::S3ToPg => PgPrivateInventory::new(pool)
                    .scan_page(&store, scope)
                    .await
                    .map(|r| r.is_some()),
                Direction::PgToS3 => PgRegisteredAudit::new(pool)
                    .scan_page(&store, scope)
                    .await
                    .map(|r| r.is_some()),
            }
        };
        match outcome {
            Ok(true) => private_response(Redirect::to(PATH), &[]).into_response(),
            Ok(false) => deny(StatusCode::CONFLICT),
            Err(error) => deny(code(error)),
        }
    }

    pub(super) fn router() -> Router<AuthState> {
        Router::new().route("/admin/storage/reconciliation", get(page)).route(
            "/admin/storage/reconciliation/run",
            post(run).layer(DefaultBodyLimit::max(4096)),
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn g09_forms_only_have_bounded_actions_and_no_private_keys() {
            let card = render_card(
                "csrf",
                Direction::PgToS3,
                PrivateScope::Chapter,
                "status",
                false,
                false,
            );
            assert!(card.contains("name=\"csrf_token\""));
            assert!(card.contains("name=\"action\" value=\"step\""));
            assert!(!card.contains("delete"));
            assert!(!card.contains("object_key"));
            assert!(render_card(
                "csrf",
                Direction::PgToS3,
                PrivateScope::ChapterAsset,
                "status",
                false,
                true
            )
            .contains("value=\"resume_exhausted\""));
            assert!(!render_card(
                "csrf",
                Direction::S3ToPg,
                PrivateScope::Chapter,
                "status",
                false,
                true
            )
            .contains("resume_exhausted"));
        }

        #[test]
        fn g09_scope_is_fixed_no_user_selected_s3_prefix() {
            assert!(scope("private/chapter-versions/").is_none());
            assert!(scope("cover").is_none());
            assert!(scope("chapter_asset").is_some());
            assert!(direction("pg_to_s3").is_some());
            assert!(direction("delete").is_none());
        }
    }
}

#[cfg(feature = "s3-paid")]
pub(super) fn router() -> Router<AuthState> {
    enabled::router()
}
