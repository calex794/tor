//! S049 shared HTTP form contracts and presentation-only fragments.
//! Never select or grant moderation rights.
use super::escape_html;
use serde::Deserialize;
use tor_accounts::services::security::generate_session_token;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AppealForm {
    pub(super) csrf_token: String,
    pub(super) command_key: String,
    pub(super) reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DecisionForm {
    pub(super) csrf_token: String,
    pub(super) decision: String,
    pub(super) note: String,
}

/// Explicit destinations retain independent Book/FREE/PAID handlers.
#[derive(Clone, Copy)]
pub(super) enum AppealTarget {
    Book,
    Free,
    Paid,
}

impl AppealTarget {
    fn author_route(self) -> &'static str {
        match self {
            Self::Book => "comment-appeals",
            Self::Free => "free-comment-appeals",
            Self::Paid => "paid-comment-appeals",
        }
    }

    fn admin_route(self) -> &'static str {
        match self {
            Self::Book => "book-comment-appeals",
            Self::Free => "free-comment-appeals",
            Self::Paid => "paid-comment-appeals",
        }
    }
}

pub(super) fn state_label(state: &str) -> &'static str {
    match state {
        "pending" => "Ожидает рассмотрения",
        "denied" => "Скрытие оставлено в силе",
        "restored" => "Комментарий восстановлен",
        _ => "Неизвестное состояние",
    }
}

pub(super) fn submit_form(target: AppealTarget, id: i64, csrf: &str, key: &str) -> String {
    let route = target.author_route();
    let csrf = escape_html(csrf);
    let key = escape_html(key);
    format!(
        r#"<form method="post" action="/me/{route}/{id}">
                <label>Обоснование апелляции (20–1000 символов)
                <textarea name="reason" minlength="20" maxlength="1000" required></textarea></label>
                <input type="hidden" name="csrf_token" value="{csrf}">
                <input type="hidden" name="command_key" value="{key}">
                <button type="submit">Подать апелляцию</button></form>"#
    )
}

pub(super) fn decision_form(target: AppealTarget, id: i64, csrf: &str) -> String {
    let route = target.admin_route();
    let csrf = escape_html(csrf);
    format!(
        r#"<form method="post" action="/odmen/{route}/{id}/decision">
            <input type="hidden" name="csrf_token" value="{csrf}">
            <label>Мотивировка решения <input name="note" maxlength="500" required></label>
            <button name="decision" value="denied" type="submit">Оставить скрытым</button>
            <button name="decision" value="restored" type="submit">Восстановить комментарий</button>
            </form>"#
    )
}

/// Render only the action for an already authorized author-history row.
/// An existing state always takes precedence over current visibility.
pub(super) fn author_action(
    target: AppealTarget,
    id: i64,
    state: Option<&str>,
    visibility: &str,
    csrf: &str,
) -> String {
    if let Some(state) = state {
        return format!("<p>{}</p>", state_label(state));
    }
    if visibility == "hidden" {
        return submit_form(target, id, csrf, &generate_session_token());
    }
    match target {
        AppealTarget::Book => "<p>Комментарий уже не скрыт; апелляция недоступна.</p>",
        AppealTarget::Free | AppealTarget::Paid => "<p>Комментарий уже не скрыт.</p>",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_targets_preserve_their_exact_author_and_admin_form_markup() {
        for (target, author, admin) in [
            (
                AppealTarget::Book,
                "comment-appeals",
                "book-comment-appeals",
            ),
            (
                AppealTarget::Free,
                "free-comment-appeals",
                "free-comment-appeals",
            ),
            (
                AppealTarget::Paid,
                "paid-comment-appeals",
                "paid-comment-appeals",
            ),
        ] {
            let expected_submit = format!(
                r#"<form method="post" action="/me/{author}/17">
                <label>Обоснование апелляции (20–1000 символов)
                <textarea name="reason" minlength="20" maxlength="1000" required></textarea></label>
                <input type="hidden" name="csrf_token" value="csrf">
                <input type="hidden" name="command_key" value="key">
                <button type="submit">Подать апелляцию</button></form>"#
            );
            assert_eq!(submit_form(target, 17, "csrf", "key"), expected_submit);
            let expected_decision = format!(
                r#"<form method="post" action="/odmen/{admin}/23/decision">
            <input type="hidden" name="csrf_token" value="csrf">
            <label>Мотивировка решения <input name="note" maxlength="500" required></label>
            <button name="decision" value="denied" type="submit">Оставить скрытым</button>
            <button name="decision" value="restored" type="submit">Восстановить комментарий</button>
            </form>"#
            );
            assert_eq!(decision_form(target, 23, "csrf"), expected_decision);
        }
    }

    #[test]
    fn arbitrary_attribute_values_are_escaped_not_interpreted_as_markup() {
        let unsafe_value = r#""<script>&"#;
        let escaped = escape_html(unsafe_value);
        for target in [AppealTarget::Book, AppealTarget::Free, AppealTarget::Paid] {
            let submit = submit_form(target, 1, unsafe_value, unsafe_value);
            let decision = decision_form(target, 1, unsafe_value);
            for html in [&submit, &decision] {
                assert!(!html.contains("<script>"));
                assert!(html.contains(&format!(r#"name="csrf_token" value="{escaped}""#)));
            }
            assert!(submit.contains(&format!(r#"name="command_key" value="{escaped}""#)));
        }
    }

    #[test]
    fn state_labels_keep_unknown_fallback() {
        assert_eq!(state_label("pending"), "Ожидает рассмотрения");
        assert_eq!(state_label("denied"), "Скрытие оставлено в силе");
        assert_eq!(state_label("restored"), "Комментарий восстановлен");
        assert_eq!(state_label("<script>"), "Неизвестное состояние");
    }

    #[test]
    fn author_action_preserves_state_precedence_and_exact_target_form() {
        for target in [AppealTarget::Book, AppealTarget::Free, AppealTarget::Paid] {
            assert_eq!(
                author_action(target, 17, Some("pending"), "hidden", "csrf"),
                "<p>Ожидает рассмотрения</p>"
            );
            assert_eq!(
                author_action(target, 17, Some("<script>"), "hidden", "csrf"),
                "<p>Неизвестное состояние</p>"
            );
            let unavailable = match target {
                AppealTarget::Book => "<p>Комментарий уже не скрыт; апелляция недоступна.</p>",
                AppealTarget::Free | AppealTarget::Paid => "<p>Комментарий уже не скрыт.</p>",
            };
            for visibility in ["visible", "unknown"] {
                assert_eq!(
                    author_action(target, 17, None, visibility, "csrf"),
                    unavailable
                );
            }
            let html = author_action(target, 17, None, "hidden", "csrf");
            let key = html
                .split_once(r#"name="command_key" value=""#)
                .unwrap()
                .1
                .split('"')
                .next()
                .unwrap();
            assert_eq!(key.len(), 43);
            assert!(key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
            assert_eq!(html, submit_form(target, 17, "csrf", key));
        }
    }

    fn form_request(body: &'static str) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::builder()
            .method("POST")
            .header(
                axum::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(axum::body::Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn shared_submission_form_keeps_required_fields_and_rejects_unknown_fields() {
        use axum::extract::{Form, FromRequest};

        let Form(form) = Form::<AppealForm>::from_request(
            form_request("csrf_token=csrf&command_key=key&reason=+reason+"),
            &(),
        )
        .await
        .unwrap();
        assert_eq!(form.csrf_token, "csrf");
        assert_eq!(form.command_key, "key");
        assert_eq!(form.reason, " reason ");
        for malformed in [
            "command_key=key&reason=reason",
            "csrf_token=csrf&reason=reason",
            "csrf_token=csrf&command_key=key",
            "csrf_token=csrf&command_key=key&reason=reason&extra=1",
            "csrf_token=csrf&command_key=key&reason=reason&reason=other",
        ] {
            assert!(Form::<AppealForm>::from_request(form_request(malformed), &())
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn shared_decision_form_keeps_required_fields_and_rejects_unknown_fields() {
        use axum::extract::{Form, FromRequest};

        let Form(form) = Form::<DecisionForm>::from_request(
            form_request("csrf_token=csrf&decision=restored&note=+note+"),
            &(),
        )
        .await
        .unwrap();
        assert_eq!(form.csrf_token, "csrf");
        assert_eq!(form.decision, "restored");
        assert_eq!(form.note, " note ");
        for malformed in [
            "decision=restored&note=note",
            "csrf_token=csrf&note=note",
            "csrf_token=csrf&decision=restored",
            "csrf_token=csrf&decision=restored&note=note&extra=1",
            "csrf_token=csrf&decision=restored&note=note&decision=denied",
        ] {
            assert!(Form::<DecisionForm>::from_request(form_request(malformed), &())
                .await
                .is_err());
        }
    }
}
