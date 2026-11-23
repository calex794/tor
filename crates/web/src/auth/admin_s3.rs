//! S3-ADMIN: operator-only connection form. Never return raw credentials.
//! A verified connection does not enable authors, readers, migration or GC.
use super::AuthState;
use crate::{
    auth_cookies::{deny, private_response},
    pages,
};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::time::Duration;
use tor_accounts::services::security::session_token_digest;
use tor_storage::repositories::admin_s3_profiles::{
    valid_access_key, valid_bucket, valid_secret_key, AdminS3Error, PgAdminS3Profiles,
    YANDEX_ENDPOINT, YANDEX_REGION,
};
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionForm {
    csrf_token: String,
    bucket: String,
    access_key_id: String,
    secret_access_key: String,
}

fn local_http_origin(origin: &str) -> bool {
    origin.parse::<Uri>().ok().is_some_and(|uri| {
        uri.scheme_str() == Some("http") && matches!(uri.host(), Some("127.0.0.1" | "localhost"))
    })
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/admin/storage/s3", get(page).post(save))
}

async fn operator(state: &AuthState, headers: &HeaderMap) -> Result<[u8; 32], StatusCode> {
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

async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if let Err(status) = operator(&state, &headers).await {
        return deny(status);
    }
    let profiles = PgAdminS3Profiles::new(state.accounts.pool().clone());
    let record = match profiles.overview().await {
        Ok(value) => value,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookie_headers) = state.cookies.csrf_for_page(&headers);
    let csrf = pages::escape_html(&csrf);
    let status = if let Some(info) = record {
        format!(
            "<p><strong>Проверено соединение:</strong> Yandex Object Storage / \
             <code>{}</code> · {}.</p>\
             <p>Профиль сохранён в зашифрованном виде. \
             <strong>Публикация и переключение хранилища пока не включены.</strong></p>",
            pages::escape_html(&info.bucket),
            pages::escape_html(&info.verified_utc),
        )
    } else {
        "<p>Подключений ещё нет. Новые материалы по-прежнему используют прежнюю настройку TOR.</p>"
            .to_string()
    };
    let key_message = if std::env::var_os("TOR_S3_ADMIN_MASTER_KEY").is_some() {
        "На сервере задан ключ шифрования. Для сохранения он должен быть корректным 32-байтным base64."
    } else {
        "Сохранение заблокировано: на сервере ещё не задан TOR_S3_ADMIN_MASTER_KEY."
    };
    let html = format!(
        "<section class=\"section\"><div class=\"container\">\
         <h1>Подключение S3</h1>\
         <p>Яндекс S3. Адрес сервиса и регион настроены автоматически.</p>\
         {status}\
         <p>{key_message}</p>\
         <form method=\"post\" action=\"/admin/storage/s3\" autocomplete=\"off\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
         <label>Имя бакета <input name=\"bucket\" value=\"tore\" \
         maxlength=\"63\" required autocomplete=\"off\"></label>\
         <label>Access Key ID <input type=\"password\" name=\"access_key_id\" \
         maxlength=\"128\" required autocomplete=\"new-password\"></label>\
         <label>Secret Access Key <input type=\"password\" name=\"secret_access_key\" \
         maxlength=\"256\" required autocomplete=\"new-password\"></label>\
         <button type=\"submit\">Проверить и сохранить</button></form>\
         <p>Проверка выполняет только безопасный запрос HEAD к бакету. \
         Она не загружает и не удаляет файлы и не доказывает неизменяемость \
         объектов, настройку браузера или возможность восстановления.</p>\
         <p>Ключи после отправки никогда не выводятся обратно. \
         Изменить их можно только повторным вводом обоих ключей.</p>\
         <p>Endpoint: <code>{YANDEX_ENDPOINT}</code>. Регион: <code>{YANDEX_REGION}</code>.</p>\
         </div></section>"
    );
    private_response(
        pages::layout("Подключение S3", "Хранилище TOR", &html),
        &cookie_headers,
    )
}

async fn save(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<ConnectionForm>,
) -> Response {
    // The form is moved into zeroizing wrappers as soon as it is decoded.
    // No debug/trace/error response may format their contents.
    let access = Zeroizing::new(form.access_key_id);
    let secret = Zeroizing::new(form.secret_access_key);
    let bucket = form.bucket;
    let csrf = form.csrf_token;
    let digest = match operator(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &csrf) {
        return deny(StatusCode::FORBIDDEN);
    }
    if !state.cookies.secure && !local_http_origin(&state.cookies.origin) {
        // Secrets must never travel over plaintext non-loopback HTTP.
        return deny(StatusCode::FORBIDDEN);
    }
    if !valid_bucket(&bucket) || !valid_access_key(&access) || !valid_secret_key(&secret) {
        return deny(StatusCode::BAD_REQUEST);
    }
    // Refuse to use submitted S3 credentials if they cannot be encrypted.
    if !PgAdminS3Profiles::master_key_ready() {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    // Never inherit GitHub/VPS credentials when testing the *submitted* pair.
    // Use the fixed Yandex endpoint to prevent credential-exfiltrating SSRF.
    let credentials = aws_sdk_s3::config::Credentials::new(
        access.as_str(),
        secret.as_str(),
        None,
        None,
        "tor-admin-yandex",
    );
    let shared = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(YANDEX_REGION))
        .credentials_provider(credentials)
        .load()
        .await;
    let client = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::config::Builder::from(&shared)
            .endpoint_url(YANDEX_ENDPOINT)
            .force_path_style(true)
            .build(),
    );
    let verified = tokio::time::timeout(
        Duration::from_secs(7),
        client.head_bucket().bucket(&bucket).send(),
    )
    .await;
    if !matches!(verified, Ok(Ok(_))) {
        // Never show provider debug messages: they may contain auth metadata.
        return deny(StatusCode::BAD_GATEWAY);
    }
    let profiles = PgAdminS3Profiles::new(state.accounts.pool().clone());
    match profiles
        .save_verified(&digest, &bucket, &access, &secret)
        .await
    {
        Ok(()) => private_response(Redirect::to("/admin/storage/s3"), &[]),
        Err(AdminS3Error::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AdminS3Error::MasterKeyMissing | AdminS3Error::Unavailable) => {
            deny(StatusCode::SERVICE_UNAVAILABLE)
        }
        Err(AdminS3Error::Forbidden) => deny(StatusCode::FORBIDDEN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn form_fields_are_password_fields_and_storage_is_not_activated() {
        let form = r#"<input type=\"password\" name=\"secret_access_key\">"#;
        assert!(form.contains("password"));
        assert!(!form.contains("not-the-secret"));
        assert!(YANDEX_ENDPOINT.starts_with("https://"));
        assert!(local_http_origin("http://127.0.0.1:3098"));
        assert!(local_http_origin("http://localhost:3000"));
        assert!(!local_http_origin("http://localhost.evil.example"));
        assert!(!local_http_origin("http://example.com"));
    }
}
