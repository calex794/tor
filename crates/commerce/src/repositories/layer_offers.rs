//! S031: read-only advertised Layer checkout terms and exact current Books.
//! This is a quote, not an entitlement or a financial posting.
use super::layers::PgLayers;
use crate::domain::layers::{
    LayerBookCandidate, LayerError, LayerOfferQuote, LayerOwnerOfferState, LayerTermKind,
};
use tor_contracts::paid_delivery::PositiveId;

impl PgLayers {
    /// Existing trusted internal publisher remains available to code callers.
    pub async fn publish_offer(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        price_minor: i64,
        lifetime: bool,
    ) -> Result<i64, LayerError> {
        self.publish_offer_internal(actor, layer, price_minor, lifetime, None)
            .await
    }

    /// Browser CAS binds the owner, current composition, and previous offer.
    /// Replaying a submitted form after success cannot publish another revision.
    pub async fn publish_offer_if_current(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        expected_version: PositiveId,
        expected_offer: Option<PositiveId>,
        price_minor: i64,
        lifetime: bool,
    ) -> Result<i64, LayerError> {
        self.publish_offer_internal(
            actor, layer, price_minor, lifetime, Some((expected_version, expected_offer)),
        )
        .await
    }

    /// Private owner-facing values, never a public quote or paid entitlement.
    pub async fn owner_offer_state(
        &self,
        actor: PositiveId,
        layer: PositiveId,
    ) -> Result<LayerOwnerOfferState, LayerError> {
        let state: Option<(i64, Option<i64>, Option<String>, Option<i64>)> =
            sqlx::query_as(
                "SELECT l.current_version_id,l.current_offer_id,o.term_kind,o.price_minor
                 FROM commerce_layers l
                 LEFT JOIN commerce_layer_offers o
                   ON o.id=l.current_offer_id AND o.layer_id=l.id
                 WHERE l.id=$1 AND l.owner_user_id=$2 AND l.status='active'",
            )
            .bind(layer.get())
            .bind(actor.get())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let Some((current_version_id, current_offer_id, kind, price_minor)) = state else {
            return Err(LayerError::Inaccessible);
        };
        let term_kind = match kind.as_deref() {
            Some("prepaid_30d") => Some(LayerTermKind::Prepaid30Days),
            Some("lifetime") => Some(LayerTermKind::Lifetime),
            None => None,
            _ => return Err(LayerError::Unavailable),
        };
        if current_offer_id.is_some() != price_minor.is_some()
            || current_offer_id.is_some() != term_kind.is_some()
        {
            return Err(LayerError::Unavailable);
        }
        Ok(LayerOwnerOfferState {
            current_version_id,
            current_offer_id,
            term_kind,
            price_minor,
        })
    }

    async fn publish_offer_internal(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        price_minor: i64,
        lifetime: bool,
        expected: Option<(PositiveId, Option<PositiveId>)>,
    ) -> Result<i64, LayerError> {
        if price_minor <= 0 {
            return Err(LayerError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let locked: Option<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT current_version_id,current_offer_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status='active' FOR UPDATE",
        )
        .bind(layer.get())
        .bind(actor.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let Some((version, previous_offer)) = locked else {
            return Err(LayerError::Inaccessible);
        };
        if let Some((expected_version, expected_offer)) = expected {
            if version != expected_version.get()
                || previous_offer != expected_offer.map(PositiveId::get)
            {
                return Err(LayerError::StaleOffer);
            }
            // A published price must cover the complete actual composition;
            // older/paid versions are not rewritten by a new offer.
            let (count, unavailable): (i64, bool) = sqlx::query_as(
                "SELECT COUNT(*), COALESCE(BOOL_OR(b.admin_hold OR b.is_deleted),FALSE)
                 FROM commerce_layer_version_books vb
                 JOIN books b ON b.id=vb.book_id
                 WHERE vb.layer_version_id=$1",
            )
            .bind(version)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
            if !(1..=100).contains(&count) || unavailable {
                return Err(LayerError::Inaccessible);
            }
        }
        let previous_kind: Option<String> = sqlx::query_scalar(
            "SELECT o.term_kind FROM commerce_layers l
             JOIN commerce_layer_offers o ON o.id=l.current_offer_id
             WHERE l.id=$1",
        )
        .bind(layer.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        // An already offered lifetime entitlement cannot silently become
        // a 30-day subscription on the same layer.
        if previous_kind.as_deref() == Some("lifetime") && !lifetime {
            return Err(LayerError::Invalid);
        }
        let offer: i64 = sqlx::query_scalar(
            "INSERT INTO commerce_layer_offers(layer_id,revision,term_kind,price_minor)
             SELECT $1,COALESCE(MAX(revision),0)+1,$2,$3
             FROM commerce_layer_offers WHERE layer_id=$1 RETURNING id",
        )
        .bind(layer.get())
        .bind(if lifetime { "lifetime" } else { "prepaid_30d" })
        .bind(price_minor)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        sqlx::query("UPDATE commerce_layers SET current_offer_id=$2 WHERE id=$1")
            .bind(layer.get())
            .bind(offer)
            .execute(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
        tx.commit().await.map_err(|_| LayerError::Unavailable)?;
        Ok(offer)
    }

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
        let rows: Vec<(i64, String, bool, bool)> = sqlx::query_as(
            "SELECT b.id,b.title,b.admin_hold,b.is_deleted FROM commerce_layer_version_books vb
             JOIN books b ON b.id=vb.book_id
             WHERE vb.layer_version_id=$1
             ORDER BY vb.book_id LIMIT 101",
        )
        .bind(version_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if rows.is_empty()
            || rows.len() > 100
            || rows.iter().any(|(_, _, held, deleted)| *held || *deleted)
        {
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
                .map(|(book_id, title, _, _)| LayerBookCandidate { book_id, title })
                .collect(),
        })
    }
}
