//! Read-only public Book purchase alternative; no money paths.
use super::{pages, AppState};
use axum::{
    http::{HeaderMap, StatusCode},
    response::Html,
};

pub(super) async fn attach(
    state: &AppState,
    headers: &HeaderMap,
    catalog: &tor_books::repositories::PgCatalog,
    book_id: i64,
    html: Html<String>,
) -> Result<Html<String>, StatusCode> {
    if !state.paid_batch_browser_enabled
        || std::env::var("TOR_LAYER_PAGES_ENABLED").as_deref() != Ok("1")
        || std::env::var("TOR_LAYER_CHECKOUT_ENABLED").as_deref() != Ok("1")
    {
        return Ok(html);
    }
    let viewer = match state.follow_control.as_ref() {
        Some(control) => control.verified_reader(headers).await?,
        None => None,
    };
    let package = tor_commerce::repositories::layer_storefront::exclusive_package_link(
        &catalog.pool(),
        book_id,
        None,
        viewer,
    )
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let Some(package) = package else {
        return Ok(html);
    };
    let label = pages::escape_html(&package.title);
    let explanation = package.until_utc.as_ref().map_or_else(
        || "Новые платные главы продаются только через пакет.".to_owned(),
        |date| {
            format!(
                "Часть платных глав доступна через пакет до {} UTC; остальные могут продаваться отдельно.",
                pages::escape_html(date)
            )
        },
    );
    Ok(Html(
        html.0.replace(
            "<!--BOOK_RELATED-->",
            &format!(
                "<section class=\"book-section-card\" aria-label=\"Способ покупки платных глав\">
                 <h2>Доступно через пакет</h2><p>{explanation}</p>
                 <a class=\"ui-button ui-button--primary\" href=\"/layers/{}\">Посмотреть пакет «{label}»</a>
                 </section><!--BOOK_RELATED-->",
                package.layer_id,
            ),
        ).replace(
            &format!("<a class=\"ui-button ui-button--ghost\" href=\"/me/books/{book_id}/buy-chapters\">Купить несколько глав</a>"),
            "",
        ),
    ))
}
