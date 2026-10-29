//! S034: deliberate, non-JavaScript internal-wallet confirmation for a single chapter.
use super::{escape_html, layout};
use axum::response::Html;
use tor_commerce::repositories::checkout::PublishedChapterOffer;

pub fn chapter_purchase_form(
    offer: &PublishedChapterOffer,
    csrf: &str,
    command_key: &str,
) -> Html<String> {
    let book = escape_html(&offer.book_title);
    let chapter = escape_html(&offer.chapter_title);
    let price = format!("{},{:02}", offer.price_minor / 100, offer.price_minor % 100);
    let body = format!(
        r#"<section class="section auth-section">
  <div class="container">
    <div class="auth-card reader-access-page">
      <p class="eyebrow">Подтверждение покупки</p>
      <h1>Купить главу</h1>
      <p><strong>{book}</strong> · Глава {chapter_no}. {chapter}</p>
      <p class="purchase-amount">К списанию из внутреннего кошелька: <strong>{price} ₽</strong></p>
      <p>Будет приобретена редакция, указанная на момент подтверждения. Новые редакции продаются отдельно.</p>
      <form method="post" action="/me/chapters/{id}/buy">
        <input type="hidden" name="expected_version_id" value="{version}">
        <input type="hidden" name="expected_price_minor" value="{minor}">
        <input type="hidden" name="command_key" value="{key}">
        <input type="hidden" name="csrf_token" value="{csrf}">
        <button class="button" type="submit">Подтвердить покупку за {price} ₽</button>
      </form>
      <p>Покупка будет выполнена только при достаточном балансе. Внешние платежи здесь не принимаются.</p>
      <p><a href="/books/{book_id}#chapters">Вернуться к книге</a> ·
         <a href="/me/purchases">Мои покупки</a></p>
    </div>
  </div>
</section>"#,
        book = book,
        chapter_no = offer.chapter_no,
        chapter = chapter,
        price = price,
        id = offer.chapter_id,
        version = offer.version_id,
        minor = offer.price_minor,
        key = escape_html(command_key),
        csrf = escape_html(csrf),
        book_id = offer.book_id,
    );
    layout(
        "Подтверждение покупки",
        "Покупка одной опубликованной главы.",
        &body,
    )
}

/// A declined payment never claims that money was taken.
pub fn chapter_purchase_problem(chapter_id: i64, message: &str) -> Html<String> {
    let body = format!(
        r#"<section class="section auth-section">
  <div class="container"><div class="auth-card reader-access-page">
    <h1>Покупка не выполнена</h1>
    <p role="alert">{message}</p>
    <a class="button" href="/me/chapters/{chapter_id}/buy">Открыть предложение заново</a>
    <a href="/me/purchases">Мои покупки</a>
  </div></div>
</section>"#,
        message = escape_html(message),
    );
    layout("Покупка не выполнена", "Средства не списаны.", &body)
}
