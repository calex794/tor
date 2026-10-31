//! S005: a public paywall for published paid-chapter metadata only.
use super::{escape_html, layout};
use axum::response::Html;
use tor_reading::services::PublicPaidChapterPreview;

/// No paid body, purchase token, signed URL, S3 key or false purchase form.
/// The real checkout is separately authenticated and feature-gated in D.
pub fn paid_chapter_paywall(chapter: &PublicPaidChapterPreview) -> Html<String> {
    paid_chapter_paywall_with_checkout(chapter,0,false)
}

pub fn paid_chapter_paywall_with_checkout(
    chapter: &PublicPaidChapterPreview,
    chapter_id: u64,
    checkout_enabled: bool,
) -> Html<String> {
    let buy_link = if checkout_enabled && chapter.price_minor>0 && chapter_id>0 {
        format!("<a class=\"button\" href=\"/me/books/{}/buy-chapter/{chapter_id}\">Купить главу</a>",chapter.book_id)
    } else { String::new() };
    let book = escape_html(&chapter.book_title);
    let title = escape_html(&chapter.chapter_title);
    let price = if chapter.price_minor > 0 {
        format!(
            "Цена главы: {},{:02} ₽.",
            chapter.price_minor / 100,
            chapter.price_minor % 100
        )
    } else {
        String::from("Стоимость главы пока не указана.")
    };
    let html = format!(
        r#"<section class="section reader-page reader-access-page">
  <div class="container">
    <div class="auth-card" role="region" aria-labelledby="paid-chapter-title">
      <p class="eyebrow">Платная глава</p>
      <h1 id="paid-chapter-title">Для чтения нужен доступ</h1>
      <p><strong>{book}</strong> · Глава {}. {title}</p>
      <p>{price} Текст главы доступен только после подтверждения покупки.</p>
      <p>Войдите в аккаунт, чтобы проверить купленные главы и доступные способы получения доступа.</p>
      <nav aria-label="Действия с платной главой">
        {buy_link}
        <a class="button" href="/login">Войти</a>
        <a class="button button--small" href="/me">Мой кабинет</a>
        <a href="/books/{}#chapters">Вернуться к книге</a>
      </nav>
    </div>
  </div>
</section>"#,
        chapter.chapter_no, chapter.book_id
    );
    layout(
        "Платная глава",
        "Для чтения платной главы необходим подтверждённый доступ.",
        &html,
    )
}
