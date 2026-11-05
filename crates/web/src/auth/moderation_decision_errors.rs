//! Shared private HTML explanation for site-moderator decision failures.
//! Never include Book metadata, CSRF values or reviewer identity in failures.
use crate::{auth_cookies::private_response, pages};
use axum::{http::StatusCode, response::Response};

/// Display a non-leaking explanation for moderation decisions that lost a
/// revision race or failed validation. Preserve the real HTTP error status.
pub(crate) fn decision_error(status: StatusCode, return_to: &'static str) -> Response {
    let (heading, explanation) = match status {
        StatusCode::CONFLICT => (
            "Заявка уже изменилась",
            "Другой модератор уже принял решение либо ревизия книги изменилась. Обновите очередь.",
        ),
        StatusCode::BAD_REQUEST => (
            "Проверьте решение",
            "Для отказа требуется причина; также проверьте допустимость параметров заявки.",
        ),
        StatusCode::FORBIDDEN => (
            "Доступ запрещён",
            "Для этого действия необходимы действующие права модератора сайта.",
        ),
        StatusCode::NOT_FOUND => (
            "Заявка не найдена",
            "Она могла быть рассмотрена или удалена из очереди.",
        ),
        _ => (
            "Не удалось принять решение",
            "Сервер не подтвердил действие. Ничего не считайте одобренным до повторной проверки.",
        ),
    };
    private_response(
        (
            status,
            pages::layout(
                heading,
                "Закрытый результат рассмотрения заявки.",
                &format!(
                    "<section class=\"me-page\"><div class=\"container\">
                     <h1>{heading}</h1><p>{explanation}</p>
                     <p><a href=\"{return_to}\">Вернуться в очередь</a></p>
                     </div></section>"
                ),
            ),
        ),
        &[],
    )
}
