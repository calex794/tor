//! S025: opt-in read-only projection of ledger-confirmed CDN cover URLs.
//! Never copy to S3 or purge its cache on an HTML request.
use crate::cover_delivery::CoverDelivery;
use axum::{http::StatusCode, response::Html};
use tor_books::repositories::cover_slots::PublicCoverKind;

pub(super) async fn render_confirmed(
    enabled: bool,
    delivery: Option<&CoverDelivery>,
    html: Html<String>,
    ids: &[u64],
    kind: PublicCoverKind,
) -> Result<Html<String>, StatusCode> {
    if !enabled || ids.is_empty() {
        return Ok(html);
    }
    let delivery = delivery.ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let urls = delivery
        .current_cdn_urls(ids, kind)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let mut result = html.0;
    let suffix = match kind {
        PublicCoverKind::Card => "card.webp",
        PublicCoverKind::Main => "main.webp",
    };
    for &id in ids {
        if let Some(url) = urls.get(&id) {
            let old = format!("src=\"/covers/{id}/{suffix}\"");
            let new = format!("src=\"{url}\"");
            result = result.replace(&old, &new);
        }
    }
    Ok(Html(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_keeps_legacy_images_and_missing_registry_fails_closed() {
        let original = Html("<img src=\"/covers/1/card.webp\">".to_owned());
        let actual = render_confirmed(false, None, original.clone(), &[1], PublicCoverKind::Card)
            .await
            .unwrap();
        assert_eq!(actual.0, original.0);
        assert_eq!(
            render_confirmed(true, None, original, &[1], PublicCoverKind::Card)
                .await
                .err(),
            Some(StatusCode::SERVICE_UNAVAILABLE)
        );
    }
}
