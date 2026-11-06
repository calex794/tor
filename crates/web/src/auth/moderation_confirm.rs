//! Explicit read-only confirmation step for site moderator decisions.
//! A preview never changes a Book or a cover; the final POST is independently
//! guarded by the existing role, session, CSRF, origin and revision checks.
use crate::{auth_cookies::private_response, pages};
use axum::{http::StatusCode, response::Response};

pub(super) fn was_confirmed(value: Option<&str>) -> Result<bool, StatusCode> {
    match value {
        None => Ok(false),
        Some("yes") => Ok(true),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

pub(super) struct ReviewIntent<'a> {
    pub kind: &'a str,
    pub subject_id: i64,
    pub revision: i64,
    pub action: &'a str,
    pub reason: &'a str,
    pub csrf: &'a str,
}

pub(super) fn confirmation_page(intent: ReviewIntent<'_>) -> Response {
    if intent.subject_id <= 0
        || (intent.kind != "cover" && intent.revision <= 0)
        || intent.revision < 0
        || intent.reason.len() > 2000
        || (intent.action == "reject" && intent.reason.trim().is_empty())
    {
        return private_response(StatusCode::BAD_REQUEST, &[]);
    }
    let (label, post_path, revision_field, action_field, back) = match intent.kind {
        "registration" => (
            "регистрации книги",
            format!("/moderation/books/{}/registration", intent.subject_id),
            "expected_revision",
            "action",
            "/moderation/books/registrations",
        ),
        "cover" => (
            "обложке",
            format!("/moderation/covers/{}/decision", intent.subject_id),
            "expected_slot_revision",
            "action",
            "/moderation/covers",
        ),
        "deletion" => (
            "удалению книги",
            format!("/moderation/books/{}/deletion", intent.subject_id),
            "request_revision",
            "decision",
            "/moderation/books/deletions",
        ),
        _ => return private_response(StatusCode::BAD_REQUEST, &[]),
    };
    let (verb, consequence) = match (intent.kind, intent.action) {
        ("registration", "approve") => (
            "Одобрить регистрацию",
            "Регистрация будет одобрена. Черновик не публикуется автоматически.",
        ),
        ("registration", "reject") => (
            "Отклонить регистрацию",
            "Регистрация будет отклонена; автор увидит указанную причину.",
        ),
        ("cover", "approve") => (
            "Одобрить обложку",
            "Новая обложка заменит опубликованную только после успешного решения.",
        ),
        ("cover", "reject") => (
            "Отклонить обложку",
            "Новая обложка будет отклонена; предыдущая останется опубликованной.",
        ),
        ("deletion", "approve") => (
            "Подтвердить удаление",
            "Обычное удаление завершится; сохранённые и купленные редакции останутся в библиотеках читателей.",
        ),
        ("deletion", "reject") => (
            "Отклонить удаление",
            "Заявка будет отклонена, прежняя публичная видимость книги восстановится.",
        ),
        _ => return private_response(StatusCode::BAD_REQUEST, &[]),
    };
    let reason = pages::escape_html(intent.reason);
    let csrf = pages::escape_html(intent.csrf);
    let reason_display = if intent.action == "reject" {
        format!("<p><strong>Причина отказа:</strong> {reason}</p>")
    } else {
        String::new()
    };
    let preview = if intent.kind == "cover" {
        format!(
            "<p><img src=\"/moderation/covers/{}/preview\" width=\"152\" height=\"228\" alt=\"Проверяемая обложка\"></p>",
            intent.subject_id
        )
    } else {
        String::new()
    };
    private_response(
        pages::layout(
            "Подтверждение решения модератора",
            "Окончательное действие выполняется только после отдельного подтверждения.",
            &format!(
                "<section class=\"me-page\"><div class=\"container\">\
                 <h1>Подтверждение решения по {label}</h1>\
                 <p>Заявка/книга №{id}, ревизия {rev}.</p>\
                 <p>{consequence}</p>{preview}{reason_display}\
                 <form method=\"post\" action=\"{post_path}\">\
                   <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
                   <input type=\"hidden\" name=\"{revision_field}\" value=\"{rev}\">\
                   <input type=\"hidden\" name=\"{action_field}\" value=\"{action}\">\
                   <input type=\"hidden\" name=\"reason\" value=\"{reason}\">\
                   <input type=\"hidden\" name=\"confirmed\" value=\"yes\">\
                   <button type=\"submit\">{verb}</button>\
                 </form>\
                 <p><a href=\"{back}\">Отмена — вернуться к очереди</a></p>\
                 </div></section>",
                id = intent.subject_id,
                rev = intent.revision,
                action = intent.action,
            ),
        ),
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_is_separate_and_escapes_rejection_reason() {
        assert_eq!(was_confirmed(None), Ok(false));
        assert_eq!(was_confirmed(Some("yes")), Ok(true));
        assert_eq!(
            was_confirmed(Some("false")),
            Err(StatusCode::BAD_REQUEST)
        );
        let response = confirmation_page(ReviewIntent {
            kind: "registration",
            subject_id: 91,
            revision: 3,
            action: "reject",
            reason: "<script>bad</script>",
            csrf: "token",
        });
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}
