//! SEC-TRAFFIC: privacy-safe process-local counters for administrators.
//! No IP, UA, path, cookie, user identity or speculative unique-bot counts.
use super::AuthState;
use crate::{auth_cookies::{deny, private_response}, pages};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use tor_accounts::services::security::session_token_digest;
use tor_observability::{
    services::{auth::{LoginOutcome, LoginSnapshot}, HttpSummary},
};

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/admin/security/traffic", get(page))
}

fn render(
    traps: (u64, u64, u64),
    logins: LoginSnapshot,
    http: HttpSummary,
) -> String {
    let (login_traps, registration_traps, route_traps) = traps;
    format!(
        "<section class=\"section\"><div class=\"container\">\
         <h1>Безопасность: активность запросов</h1>\
         <p><strong>Счётчики текущего процесса TOR.</strong> \
         Значения накоплены с момента запуска сервера и обнуляются при его перезапуске. \
         Это не журнал Nginx, не последние 15 минут, не число уникальных ботов \
         и не доказательство атаки.</p>\
         <h2>Подозрительные признаки</h2>\
         <table><tbody>\
         <tr><th>Запросы к адресам-ловушкам</th><td>{route_traps}</td></tr>\
         <tr><th>Заполненные скрытые поля входа</th><td>{login_traps}</td></tr>\
         <tr><th>Заполненные скрытые поля регистрации</th><td>{registration_traps}</td></tr>\
         <tr><th>Неверные данные при входе</th><td>{invalid}</td></tr>\
         <tr><th>Ограниченные попытки входа</th><td>{limited}</td></tr>\
         </tbody></table>\
         <h2>Все HTTP-запросы этого процесса</h2>\
         <table><tbody>\
         <tr><th>Всего</th><td>{requests}</td></tr>\
         <tr><th>Ответы 4xx</th><td>{client_errors}</td></tr>\
         <tr><th>Ответы 5xx</th><td>{server_errors}</td></tr>\
         <tr><th>Медленные запросы (больше секунды)</th><td>{slow}</td></tr>\
         </tbody></table>\
         <p>Чтобы увидеть реальные обращения ботов прямо сейчас, \
         нужны журналы Nginx или WAF с временем и частотой запросов. \
         По этим агрегатам нельзя определить конкретный IP либо отличить \
         поискового робота от злоумышленника.</p>\
         <p><a href=\"/admin/storage/s3\">Настройки S3</a></p>\
         </div></section>",
        invalid=logins.count(LoginOutcome::InvalidCredentials),
        limited=logins.count(LoginOutcome::RateLimited),
        requests=http.requests,
        client_errors=http.client_errors,
        server_errors=http.server_errors,
        slow=http.slower_than_one_second,
    )
}

async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    let actor = match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !actor.administrator {
        return deny(StatusCode::FORBIDDEN);
    }
    let body = render(
        super::honeypot::counts(),
        crate::observability::login_summary(),
        crate::observability::http_summary(),
    );
    private_response(pages::layout("Активность запросов", "Безопасность TOR", &body), &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_reports_measured_signals_not_fabricated_unique_bots() {
        let markup = render(
            (3, 5, 7),
            LoginSnapshot::default(),
            HttpSummary { requests: 140, client_errors: 11, server_errors: 2,
                slower_than_one_second: 9 },
        );
        assert!(markup.contains("адресам-ловушкам</th><td>7"));
        assert!(markup.contains("регистрации</th><td>5"));
        assert!(markup.contains("входа</th><td>3"));
        assert!(markup.contains("Всего</th><td>140"));
        assert!(markup.contains("Это не журнал Nginx"));
        for forbidden in ["<script", "secret_access_key", "session_token=", "real_ip=", "User-Agent:"] {
            assert!(!markup.contains(forbidden));
        }
    }
}
