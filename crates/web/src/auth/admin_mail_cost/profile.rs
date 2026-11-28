//! No secret is ever rendered or passed into the public response model.
use super::{pages, AuthState};
use axum::http::{StatusCode, Uri};
use serde::Deserialize;
use tor_mail::{
    domain::postbox_profile::ProfileMasterKey,
    repositories::postbox_profile::{PgPostboxProfile, ProfileError, ProfileView},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProfileForm {
    pub csrf_token: String,
    pub revision: i64,
    pub identity_id: String,
    pub sender: String,
    pub access: String,
    pub secret: String,
    pub current_password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProbeForm {
    pub csrf_token: String,
    pub revision: i64,
}

pub(super) fn store(state: &AuthState) -> PgPostboxProfile {
    PgPostboxProfile::new(state.accounts.pool().clone())
}

pub(super) fn status(error: ProfileError) -> StatusCode {
    match error {
        ProfileError::Invalid => StatusCode::BAD_REQUEST,
        ProfileError::Forbidden => StatusCode::NOT_FOUND,
        ProfileError::Conflict => StatusCode::CONFLICT,
        ProfileError::StepUpRequired => StatusCode::UNAUTHORIZED,
        ProfileError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub(super) fn master_key() -> Result<ProfileMasterKey, StatusCode> {
    crate::flow_runtime::mail::profile_master_key().map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

pub(super) fn secure_origin(state: &AuthState) -> bool {
    state.cookies.secure
        || state.cookies.origin.parse::<Uri>().ok().is_some_and(|uri| {
            uri.scheme_str() == Some("http")
                && matches!(uri.host(), Some("127.0.0.1" | "localhost"))
        })
}

pub(super) fn body(view: &ProfileView, csrf: &str, key_ready: bool, enabled: bool) -> String {
    let csrf = pages::escape_html(csrf);
    let (identity, sender, summary) = match &view.identity {
        Some(identity) => (
            pages::escape_html(identity.identity_id()),
            pages::escape_html(identity.sender()),
            "Профиль сохранён зашифрованным. Проверка не включает отправку.",
        ),
        None => (String::new(), String::new(), "Профиль ещё не сохранён."),
    };
    let mut html = format!(
        "<section class=\"section\"><div class=\"container\"><h2>Подключение Postbox</h2>\
         <p>postbox.cloud.yandex.net:465 · TLS</p><p>{summary}</p>"
    );
    if !key_ready || enabled {
        html.push_str(if enabled {
            "<p>Сначала остановите отправку.</p>"
        } else {
            "<p>Сохранение недоступно: мастер-ключ или защищённое соединение не настроены.</p>"
        });
    } else {
        html.push_str(&format!(
            "<form method=\"post\" action=\"/odmen/system-mail/profile\" autocomplete=\"off\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
             <input type=\"hidden\" name=\"revision\" value=\"{}\">\
             <label>ID удостоверения Postbox <input name=\"identity_id\" maxlength=\"128\" value=\"{identity}\" required></label>\
             <label>Адрес отправителя <input type=\"email\" name=\"sender\" maxlength=\"254\" value=\"{sender}\" required></label>\
             <label>ID API-ключа <input type=\"password\" name=\"access\" maxlength=\"1024\" autocomplete=\"new-password\" required></label>\
             <label>Секрет API-ключа <input type=\"password\" name=\"secret\" maxlength=\"1024\" autocomplete=\"new-password\" required></label>\
             <label>Текущий пароль администратора <input type=\"password\" name=\"current_password\" maxlength=\"512\" autocomplete=\"current-password\" required></label>\
             <button type=\"submit\">Сохранить зашифрованный профиль</button></form>",
            view.revision,
        ));
        if view.identity.is_some() {
            html.push_str(&format!(
                "<form method=\"post\" action=\"/odmen/system-mail/probe\">\
                 <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
                 <input type=\"hidden\" name=\"revision\" value=\"{}\">\
                 <button type=\"submit\">Проверить подключение (без письма)</button></form>",
                view.revision,
            ));
        }
    }
    html.push_str(
        "<p>Ключи повторно не показываются. Для замены введите оба ключа. \
         Сохранение и проверка не разрешают рассылку.</p></div></section>",
    );
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_mail::domain::postbox_profile::ProfileIdentity;

    #[test]
    fn only_metadata_and_empty_secret_fields_are_rendered() {
        let view = ProfileView {
            revision: 3,
            identity: Some(
                ProfileIdentity::new("identity-1".into(), "sender@example.com".into()).unwrap(),
            ),
            key_version: Some(1),
        };
        let html = body(&view, "csrf<fixture>", true, false);
        assert!(html.contains("sender@example.com"));
        assert!(html.contains("csrf&lt;fixture&gt;"));
        assert!(html.contains("type=\"password\" name=\"secret\""));
        assert!(html.contains("/odmen/system-mail/probe"));
        assert!(!html.contains("name=\"enabled\""));
        assert!(!body(&view, "", false, false).contains("name=\"secret\""));
        assert!(!body(&view, "", true, true).contains("/odmen/system-mail/probe"));
    }
}
