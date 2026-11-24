//! S049 presentation-only fragments. Never select or grant moderation rights.
use super::escape_html;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_targets_preserve_their_exact_author_and_admin_form_markup() {
        for (target, author, admin) in [
            (AppealTarget::Book, "comment-appeals", "book-comment-appeals"),
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
}
