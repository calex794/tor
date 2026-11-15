//! H/UI-19: Book-row-fenced restriction on NEW direct paid-chapter sales.
//! Free chapters, old direct purchase receipts and historic package terms
//! are intentionally outside this feature. No money or rights are mutated.
use super::layers::PgLayers;
use crate::domain::layers::LayerError;
use sqlx::Row;
use tor_contracts::paid_delivery::PositiveId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackagePaidMode {
    Direct,
    First7Days,
    First14Days,
    PackageOnly,
}

impl PackagePaidMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "direct" => Some(Self::Direct),
            "7_days" => Some(Self::First7Days),
            "14_days" => Some(Self::First14Days),
            "package_only" => Some(Self::PackageOnly),
            _ => None,
        }
    }

    fn days(self) -> Option<i64> {
        match self {
            Self::First7Days => Some(7),
            Self::First14Days => Some(14),
            Self::Direct | Self::PackageOnly => None,
        }
    }

    fn is_restricted(self) -> bool {
        self != Self::Direct
    }
}

#[derive(Debug)]
pub struct PackageSaleBook {
    pub id: i64,
    pub title: String,
    pub revision: i64,
    pub restricted_here: bool,
    pub restricted_elsewhere: bool,
    pub until_utc: Option<String>,
}

#[derive(Debug)]
pub struct PackageSalePage {
    pub version_id: i64,
    pub editable: bool,
    pub books: Vec<PackageSaleBook>,
}

impl PgLayers {
    pub async fn package_sale_page(
        &self,
        owner: PositiveId,
        layer: PositiveId,
    ) -> Result<PackageSalePage, LayerError> {
        let l = sqlx::query(
            "SELECT current_version_id,
                    (status='active' AND current_offer_id IS NOT NULL) AS editable
             FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status IN ('active','hidden')",
        )
        .bind(layer.get())
        .bind(owner.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        let version: i64 = l
            .try_get("current_version_id")
            .map_err(|_| LayerError::Unavailable)?;
        let rows = sqlx::query(
            "SELECT b.id,b.title,b.package_only_revision,
                (b.package_only_layer_id=$2 AND
                 (b.package_only_until IS NULL OR b.package_only_until>now()))
                    AS restricted_here,
                (b.package_only_layer_id IS NOT NULL AND
                 b.package_only_layer_id<>$2 AND
                 (b.package_only_until IS NULL OR b.package_only_until>now()))
                    AS restricted_elsewhere,
                CASE WHEN b.package_only_layer_id=$2 AND b.package_only_until>now()
                  THEN to_char(b.package_only_until AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI')
                  ELSE NULL END AS until_utc
             FROM commerce_layer_version_books vb
             JOIN books b ON b.id=vb.book_id
             WHERE vb.layer_version_id=$1
             ORDER BY b.id LIMIT 101",
        )
        .bind(version)
        .bind(layer.get())
        .fetch_all(&self.pool)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        let mut books = Vec::new();
        for row in rows {
            books.push(PackageSaleBook {
                id: row.try_get("id").map_err(|_| LayerError::Unavailable)?,
                title: row.try_get("title").map_err(|_| LayerError::Unavailable)?,
                revision: row
                    .try_get("package_only_revision")
                    .map_err(|_| LayerError::Unavailable)?,
                restricted_here: row
                    .try_get("restricted_here")
                    .map_err(|_| LayerError::Unavailable)?,
                restricted_elsewhere: row
                    .try_get("restricted_elsewhere")
                    .map_err(|_| LayerError::Unavailable)?,
                until_utc: row
                    .try_get("until_utc")
                    .map_err(|_| LayerError::Unavailable)?,
            });
        }
        Ok(PackageSalePage {
            version_id: version,
            editable: l.try_get("editable").map_err(|_| LayerError::Unavailable)?,
            books,
        })
    }

    /// Locks package then Book in the same order as immutable composition.
    /// The Book row is also SHARE-locked by every direct monetary checkout;
    /// requests already committed can replay without charging again.
    pub async fn set_package_paid_mode(
        &self,
        owner: PositiveId,
        layer: PositiveId,
        book: PositiveId,
        expected_version: PositiveId,
        expected_revision: i64,
        mode: PackagePaidMode,
    ) -> Result<i64, LayerError> {
        if expected_revision < 0 {
            return Err(LayerError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LayerError::Unavailable)?;
        let version: i64 = sqlx::query_scalar(
            "SELECT current_version_id FROM commerce_layers
             WHERE id=$1 AND owner_user_id=$2 AND status='active'
               AND current_offer_id IS NOT NULL FOR UPDATE",
        )
        .bind(layer.get())
        .bind(owner.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        if version != expected_version.get() {
            return Err(LayerError::StaleOffer);
        }
        let row = sqlx::query(
            "SELECT b.package_only_layer_id,
                    (b.package_only_layer_id IS NOT NULL AND
                     b.package_only_layer_id<>$3 AND
                     (b.package_only_until IS NULL OR b.package_only_until>now()))
                        AS active_elsewhere,
                    b.package_only_revision,b.admin_hold,b.is_deleted,
                    b.visibility,b.visibility_status
             FROM books b WHERE b.id=$1 AND b.owner_user_id=$2
             FOR UPDATE OF b",
        )
        .bind(book.get())
        .bind(owner.get())
        .bind(layer.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::Inaccessible)?;
        let revision: i64 = row
            .try_get("package_only_revision")
            .map_err(|_| LayerError::Unavailable)?;
        if revision != expected_revision {
            return Err(LayerError::StaleOffer);
        }
        let current_layer: Option<i64> = row
            .try_get("package_only_layer_id")
            .map_err(|_| LayerError::Unavailable)?;
        let active_other: bool = row
            .try_get("active_elsewhere")
            .map_err(|_| LayerError::Unavailable)?;
        if active_other {
            return Err(LayerError::Inaccessible);
        }
        let belongs: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce_layer_version_books
             WHERE layer_version_id=$1 AND book_id=$2)",
        )
        .bind(version)
        .bind(book.get())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?;
        if !belongs {
            return Err(LayerError::StaleOffer);
        }
        if mode.is_restricted() {
            let blocked: bool = row
                .try_get("admin_hold")
                .map_err(|_| LayerError::Unavailable)?;
            let deleted: bool = row
                .try_get("is_deleted")
                .map_err(|_| LayerError::Unavailable)?;
            let visibility: String = row
                .try_get("visibility")
                .map_err(|_| LayerError::Unavailable)?;
            let status: String = row
                .try_get("visibility_status")
                .map_err(|_| LayerError::Unavailable)?;
            if blocked || deleted || visibility != "public" || status != "active" {
                return Err(LayerError::Inaccessible);
            }
            let sole_owner: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1 FROM book_members m
                    WHERE m.book_id=$1 AND m.user_id=$2 AND m.is_active AND m.role='owner'
                ) AND NOT EXISTS (
                    SELECT 1 FROM book_members m
                    WHERE m.book_id=$1 AND m.is_active
                      AND (m.user_id<>$2 OR m.role<>'owner')
                )",
            )
            .bind(book.get())
            .bind(owner.get())
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
            if !sole_owner {
                return Err(LayerError::Inaccessible);
            }
            // A Book cannot become falsely "only in this package" while its
            // current composition is also available in another package.
            // Other editors take the same Book FOR UPDATE lock on additions.
            let elsewhere: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1 FROM commerce_layers other
                    JOIN commerce_layer_version_books vb
                      ON vb.layer_version_id=other.current_version_id
                    WHERE other.id<>$1 AND other.status IN ('active','hidden')
                      AND vb.book_id=$2
                )",
            )
            .bind(layer.get())
            .bind(book.get())
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| LayerError::Unavailable)?;
            if elsewhere {
                return Err(LayerError::Invalid);
            }
        }
        let updated: i64 = sqlx::query_scalar(
            "UPDATE books
             SET package_only_layer_id=CASE WHEN $2::BOOLEAN THEN $3 ELSE NULL END,
                 package_only_until=CASE
                   WHEN $2::BOOLEAN AND $4::BIGINT IS NOT NULL
                   THEN transaction_timestamp()+($4 * INTERVAL '1 day')
                   ELSE NULL END,
                 package_only_revision=package_only_revision+1
             WHERE id=$1 AND package_only_revision=$5
             RETURNING package_only_revision",
        )
        .bind(book.get())
        .bind(mode.is_restricted())
        .bind(layer.get())
        .bind(mode.days())
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LayerError::Unavailable)?
        .ok_or(LayerError::StaleOffer)?;
        tx.commit().await.map_err(|_| LayerError::Unavailable)?;
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::PackagePaidMode;
    #[test]
    fn package_paid_mode_allowlist_is_exact() {
        assert_eq!(
            PackagePaidMode::parse("direct"),
            Some(PackagePaidMode::Direct)
        );
        assert_eq!(
            PackagePaidMode::parse("7_days"),
            Some(PackagePaidMode::First7Days)
        );
        assert_eq!(
            PackagePaidMode::parse("14_days"),
            Some(PackagePaidMode::First14Days)
        );
        assert_eq!(
            PackagePaidMode::parse("package_only"),
            Some(PackagePaidMode::PackageOnly)
        );
        for invalid in ["", "Forever", "99_days", "DIRECT", " package_only", "all"] {
            assert_eq!(PackagePaidMode::parse(invalid), None);
        }
    }
}
