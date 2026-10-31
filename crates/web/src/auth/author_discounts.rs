//! D-07: owner-only native HTML campaign control, with real Origin+CSRF.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Redirect, Response},
};
use serde::Deserialize;
use tor_commerce::repositories::discount_campaigns::{
    ActiveAuthorDiscount, DiscountCampaignError,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CampaignForm {
    csrf_token: Option<String>,
    action: String,
    discount_percent: Option<i16>,
    min_follow_days: Option<i16>,
    duration_days: Option<i16>,
}

fn status(error: DiscountCampaignError) -> StatusCode {
    match error {
        DiscountCampaignError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        DiscountCampaignError::NotFound => StatusCode::NOT_FOUND,
        DiscountCampaignError::Conflict => StatusCode::CONFLICT,
        DiscountCampaignError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn render(book_id: i64, title: &str, csrf: &str, active: Option<&ActiveAuthorDiscount>) -> Html<String> {
    let title = pages::escape_html(title);
    let action = if let Some(active) = active {
        let terms = if active.min_follow_days == 90 {
            "подписчиков от 90 дней"
        } else {
            "всех текущих подписчиков"
        };
        format!(
            "<p role=\"status\">Действующая скидка: {}% для {terms}. До {} (UTC).</p>\
             <form method=\"post\" action=\"/me/books/{book_id}/discounts\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
             <input type=\"hidden\" name=\"action\" value=\"stop\">\
             <button type=\"submit\" class=\"ui-button ui-button--ghost\">Остановить скидку</button>\
             </form>",
            active.discount_percent,
            pages::escape_html(&active.ends_on),
            pages::escape_html(csrf),
        )
    } else {
        format!(
            "<form class=\"auth-form\" method=\"post\" action=\"/me/books/{book_id}/discounts\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
             <input type=\"hidden\" name=\"action\" value=\"create\">\
             <label for=\"discount-percent\">Скидка, % (от 1 до 90)</label>\
             <input id=\"discount-percent\" type=\"number\" name=\"discount_percent\" min=\"1\" max=\"90\" value=\"20\" required>\
             <label for=\"discount-follow\">Кому доступна</label>\
             <select id=\"discount-follow\" name=\"min_follow_days\">\
             <option value=\"0\">Всем подписчикам автора</option>\
             <option value=\"90\">Подписчикам непрерывно от 90 дней</option>\
             </select>\
             <label for=\"discount-duration\">Длительность, дней (от 1 до 90)</label>\
             <input id=\"discount-duration\" type=\"number\" name=\"duration_days\" min=\"1\" max=\"90\" value=\"30\" required>\
             <button class=\"ui-button ui-button--primary\" type=\"submit\">Включить скидку</button>\
             </form>",
            pages::escape_html(csrf),
        )
    };
    let body = format!(
        "<section class=\"section auth-section\"><div class=\"container\">\
         <div class=\"auth-card\"><p class=\"eyebrow\">Книга №{book_id}</p>\
         <h1>Скидки подписчикам</h1><p>Книга: {title}</p>\
         <p>Скидка действует только для отдельной покупки платной главы.\
         При отписке и повторной подписке стаж начинается заново.\
         Изменить действующую акцию нельзя — остановите её и создайте новую.</p>\
         {action}\
         <p><a href=\"/me/books/{book_id}\">Вернуться к книге</a></p>\
         </div></div></section>"
    );
    pages::layout("Скидки подписчикам", "Управление скидками своей книги.", &body)
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !state.paid_checkout_enabled || book_id<=0 {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state,&headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"),&[]),
        Err(error) => return deny(error),
    };
    let book = match state.authoring.owned_book(actor,book_id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let active = match state.discounts.current(actor,book_id).await {
        Ok(active) => active,
        Err(error) => return deny(status(error)),
    };
    let (csrf,cookies)=state.cookies.csrf_for_page(&headers);
    private_response(render(book_id,&book.title,&csrf,active.as_ref()),&cookies)
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<CampaignForm>,
) -> Response {
    if !state.paid_checkout_enabled || book_id<=0 {
        return deny(StatusCode::NOT_FOUND);
    }
    if !state.cookies.same_origin(&headers)
        || !state.cookies.valid_csrf(&headers,form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state,&headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    let result = match form.action.as_str() {
        "create" => match (form.discount_percent,form.min_follow_days,form.duration_days) {
            (Some(percent),Some(days),Some(duration)) =>
                state.discounts.create(actor,book_id,percent,days,duration).await.map(|_|()),
            _ => Err(DiscountCampaignError::Invalid),
        },
        "stop" if form.discount_percent.is_none()
            && form.min_follow_days.is_none() && form.duration_days.is_none() =>
                state.discounts.stop(actor,book_id).await,
        _ => Err(DiscountCampaignError::Invalid),
    };
    match result {
        Ok(()) => private_response(Redirect::to(&format!("/me/books/{book_id}/discounts")),&[]),
        Err(error) => deny(status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_form_is_bounded_and_escapes_book_title() {
        let html=render(12,"<script>unsafe</script>","csrf",None).0;
        assert!(html.contains("&lt;script&gt;unsafe&lt;/script&gt;"));
        assert!(!html.contains("<script>unsafe</script>"));
        assert!(html.contains("Скидки подписчикам"));
        assert!(html.contains("name=\"min_follow_days\""));
        assert_eq!(status(DiscountCampaignError::NotFound),StatusCode::NOT_FOUND);
    }
}
