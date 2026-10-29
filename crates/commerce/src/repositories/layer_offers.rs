//! S031: read-only advertised Layer checkout terms and exact current Books.
//! This is a quote, not an entitlement or a financial posting.
use super::layers::PgLayers;
use crate::domain::layers::{
    LayerBookCandidate, LayerError, LayerOfferQuote, LayerTermKind,
};
use tor_contracts::paid_delivery::PositiveId;

impl PgLayers {
    /// Only active offers with a complete, bounded composition may be shown.
    /// The fetched immutable version may become stale: buy() must compare the
    /// current offer, version, and price again before debiting a wallet.
    pub async fn offer_quote(&self, layer: PositiveId) -> Result<LayerOfferQuote, LayerError> {
        let result: Option<(i64, String, i64, i64, i64, String)> = sqlx::query_as(
            "SELECT l.id,l.title,o.id,l.current_version_id,o.price_minor,o.term_kind
             FROM commerce_layers l
             JOIN commerce_layer_offers o ON o.id=l.current_offer_id
                AND o.layer_id=l.id
             WHERE l.id=$1 AND l.status='active'",
        )
        .bind(layer.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let Some((layer_id, title, offer_id, version_id, price_minor, kind)) = result else {
            return Err(LayerError::Inaccessible);
        };
        if price_minor <= 0 {
            return Err(LayerError::Unavailable);
        }
        let term_kind = match kind.as_str() {
            "prepaid_30d" => LayerTermKind::Prepaid30Days,
            "lifetime" => LayerTermKind::Lifetime,
            _ => return Err(LayerError::Unavailable),
        };

        // No partial/truncated composition can be presented as a sale.
        // The v1 bound fails closed rather than hiding paid Books behind UI.
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT b.id,b.title FROM commerce_layer_version_books vb
             JOIN books b ON b.id=vb.book_id
             WHERE vb.layer_version_id=$1
             ORDER BY vb.book_id LIMIT 101",
        )
        .bind(version_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if rows.is_empty() || rows.len() > 100 {
            return Err(LayerError::Inaccessible);
        }
        Ok(LayerOfferQuote {
            layer_id,
            title,
            offer_id,
            version_id,
            term_kind,
            price_minor,
            books: rows
                .into_iter()
                .map(|(book_id, title)| LayerBookCandidate { book_id, title })
                .collect(),
        })
    }
}
