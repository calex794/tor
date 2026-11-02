//! E-12 / S047: public free-reader issue reporting uses the trusted
//! account-bound PostgreSQL endpoint. Guests get no form or CSRF token.
use super::escape_html;

/// Query feedback only changes display, never mutation outcome.
pub(super) fn panel(
    chapter_id: Option<u64>,
    version_id: u64,
    csrf: Option<&str>,
    feedback: Option<&str>,
) -> String {
    let Some(id) = chapter_id else {
        return String::new();
    };
    let Some(csrf) = csrf else {
        return format!(
            "<section class=\"reader-issues reader-free-issues\" id=\"text-issues\">\
             <h2>Нашли ошибку в тексте?</h2>\
             <p><a class=\"text-link\" href=\"/read/{id}#text-issues\">\
             Войти, чтобы сообщить автору</a>. Читать можно без регистрации.</p></section>"
        );
    };
    let (message, open) = match feedback {
        Some("sent") => ("Сообщение об ошибке отправлено автору.", " open"),
        Some("repeat") => ("Такое сообщение уже было отправлено автору.", " open"),
        _ => ("", ""),
    };
    let status = if message.is_empty() {
        String::new()
    } else {
        format!("<p class=\"reader-issues__feedback\" role=\"status\">{message}</p>")
    };
    format!(
        r#"<details class="reader-issues reader-free-issues" data-free-issues id="text-issues"{open}>
          <summary>Сообщить об ошибке в тексте</summary>
          {status}
          <p>Выделите слово или короткую фразу в тексте главы. Можно сообщить об опечатке, неверном факте или оформлении.</p>
          <p class="reader-free-issues__status" data-free-issues-status role="status">Цитата пока не выбрана.</p>
          <form method="post" action="/read/{id}/text-issues" data-free-issue-form>
            <input type="hidden" name="csrf_token" value="{csrf}">
            <input type="hidden" name="return_to_public" value="1">
            <input type="hidden" name="version_id" value="{version_id}">
            <input type="hidden" name="block_index" data-free-issue-block>
            <input type="hidden" name="selection_start" data-free-issue-start>
            <input type="hidden" name="selection_end" data-free-issue-end>
            <label>Цитата
              <textarea name="quote" data-free-issue-quote readonly required maxlength="500"></textarea>
            </label>
            <label>Как правильно
              <textarea name="suggested_text" data-free-issue-suggestion required maxlength="1000"></textarea>
            </label>
            <label>Тип ошибки
              <select name="issue_type">
                <option value="typo">Опечатка</option>
                <option value="formatting">Оформление</option>
                <option value="broken_link">Ссылка</option>
                <option value="factual">Факт</option>
                <option value="continuity">Несогласованность</option>
                <option value="other">Другое</option>
              </select>
            </label>
            <button type="submit" data-free-issue-submit disabled>Отправить автору</button>
          </form>
          <p>Если выделение не работает, <a class="text-link" href="/read/{id}#text-issues">откройте обычную форму</a>.</p>
        </details>"#,
        csrf = escape_html(csrf)
    )
}

#[cfg(test)]
mod tests {
    use super::panel;

    #[test]
    fn s047_guest_does_not_receive_form_or_csrf() {
        let guest = panel(Some(101), 602, None, Some("sent"));
        assert!(guest.contains("Войти, чтобы сообщить автору"));
        assert!(!guest.contains("data-free-issue-form"));
        assert!(!guest.contains("csrf_token"));
        assert!(!guest.contains("Сообщение об ошибке отправлено"));
        assert!(panel(None, 602, Some("safe"), None).is_empty());
    }

    #[test]
    fn s047_authenticated_form_is_bound_to_exact_version_and_csrf() {
        let form = panel(Some(101), 602, Some("a\" onload=\"bad"), None);
        assert!(form.contains("action=\"/read/101/text-issues\""));
        assert!(form.contains("name=\"return_to_public\" value=\"1\""));
        assert!(form.contains("name=\"version_id\" value=\"602\""));
        assert!(form.contains("a&quot; onload=&quot;bad"));
        assert!(!form.contains("a\" onload=\"bad"));
        for name in ["block_index", "selection_start", "selection_end", "quote"] {
            assert!(form.contains(&format!("name=\"{name}\"")));
        }
        assert!(form.contains("data-free-issue-submit disabled"));
        assert!(!form.contains("Сообщение об ошибке отправлено"));
    }

    #[test]
    fn s047_feedback_is_bounded_and_untrusted_query_never_rendered() {
        assert!(panel(Some(1), 2, Some("csrf"), Some("sent")).contains("role=\"status\""));
        assert!(panel(Some(1), 2, Some("csrf"), Some("repeat")).contains("уже было"));
        assert!(!panel(Some(1), 2, Some("csrf"), Some("<script>")).contains("<script>"));
    }
}
