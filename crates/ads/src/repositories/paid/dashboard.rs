use super::*;
use sqlx::Row;
use tor_contracts::ads::AdProduct;

impl PgPaidAds {
    pub async fn daily_offers(&self) -> Result<Vec<DailyOffer>, AdError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let rows = load_daily(&mut tx, false).await?;
        tx.commit().await.map_err(db)?;
        Ok(rows)
    }
    pub async fn packages(&self) -> Result<Vec<ChapterPackage>, AdError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let rows = load_packages(&mut tx, false).await?;
        tx.commit().await.map_err(db)?;
        Ok(rows)
    }
    pub async fn admin_catalog(
        &self,
        a: &AdActor,
    ) -> Result<(Vec<DailyOffer>, Vec<ChapterPackage>), AdError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        actor(&mut tx, a, true).await?;
        let daily = load_daily(&mut tx, true).await?;
        let packages = load_packages(&mut tx, true).await?;
        tx.commit().await.map_err(db)?;
        Ok((daily, packages))
    }
    pub async fn owned_books(&self, a: &AdActor) -> Result<Vec<OwnedBook>, AdError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        actor(&mut tx, a, false).await?;
        let rows = sqlx::query(
            "SELECT b.id,b.title FROM books b JOIN rec_materializable_books v ON v.id=b.id
   WHERE b.owner_user_id=$1 AND ad_book_discoverable($1,b.id) ORDER BY b.id DESC LIMIT 100",
        )
        .bind(a.user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        let result = rows
            .iter()
            .map(|r| {
                Ok(OwnedBook {
                    id: r.try_get("id").map_err(db)?,
                    title: r.try_get("title").map_err(db)?,
                })
            })
            .collect();
        tx.commit().await.map_err(db)?;
        result
    }
    pub async fn campaigns(
        &self,
        a: &AdActor,
        product: AdProduct,
        before: Option<i64>,
    ) -> Result<Vec<Campaign>, AdError> {
        if before.is_some_and(|v| v <= 0) {
            return Err(AdError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        actor(&mut tx, a, false).await?;
        let query=match product {
   AdProduct::Daily=>"SELECT c.id,c.book_id,CASE WHEN ad_book_discoverable($1,b.id) THEN b.title ELSE 'Книга недоступна' END AS title,'daily' AS product,c.status,c.revision,f.budget_minor,f.target_units,
    f.reserved_units,f.reserved_minor,f.settled_minor,f.refunded_minor,to_char(c.starts_at AT TIME ZONE 'UTC','YYYY-MM-DD')||'T'||to_char(c.starts_at AT TIME ZONE 'UTC','HH24:MI:SS.US')||'Z' AS starts_at,
    to_char(c.ends_at AT TIME ZONE 'UTC','YYYY-MM-DD')||'T'||to_char(c.ends_at AT TIME ZONE 'UTC','HH24:MI:SS.US')||'Z' AS ends_at,
    0::BIGINT clicks,0::BIGINT opens,0::BIGINT reads,0::BIGINT library_adds,0::BIGINT purchases
    FROM ad_legacy_campaigns c JOIN finance_ad_funding f ON f.intent_id=c.funding_intent_id JOIN books b ON b.id=c.book_id
    WHERE c.advertiser_id=$1 AND ($2::BIGINT IS NULL OR c.id<$2) ORDER BY c.id DESC LIMIT 100",
   AdProduct::Chapter=>"SELECT c.id,c.promoted_book_id AS book_id,CASE WHEN ad_book_discoverable($1,b.id) THEN b.title ELSE 'Книга недоступна' END AS title,'chapter' AS product,c.status,c.revision,
    f.budget_minor,f.target_units,f.reserved_units,f.reserved_minor,f.settled_minor,f.refunded_minor,
    NULL::TEXT starts_at,NULL::TEXT ends_at,COALESCE(s.clicks,0) clicks,COALESCE(s.opens,0) opens,
    COALESCE(s.reads,0) reads,COALESCE(s.library_adds,0) library_adds,COALESCE(s.purchases,0) purchases
    FROM ad_chapter_campaigns c JOIN finance_ad_funding f ON f.intent_id=c.funding_intent_id
    JOIN books b ON b.id=c.promoted_book_id LEFT JOIN ad_campaign_stats s ON s.campaign_id=c.id
    WHERE c.advertiser_id=$1 AND ($2::BIGINT IS NULL OR c.id<$2) ORDER BY c.id DESC LIMIT 100",
  };
        let rows = sqlx::query(query)
            .bind(a.user_id)
            .bind(before)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
        let result = rows.iter().map(campaign).collect();
        tx.commit().await.map_err(db)?;
        result
    }
    pub async fn campaign(&self, a: &AdActor, id: i64) -> Result<Campaign, AdError> {
        if id <= 0 {
            return Err(AdError::Invalid);
        }
        // The owner-index keyset bounds this lookup to one row, independent of the
        // number of historical statistics. No lifetime aggregate is read here.
        let before = id.checked_add(1);
        self.campaigns(a, AdProduct::Chapter, before)
            .await?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or(AdError::NotFound)
    }
    pub async fn publisher(
        &self,
        a: &AdActor,
        before: Option<i64>,
    ) -> Result<(PublisherBalance, Vec<PublisherCredit>), AdError> {
        if before.is_some_and(|v| v <= 0) {
            return Err(AdError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        actor(&mut tx, a, false).await?;
        let row=sqlx::query("SELECT COALESCE(s.enabled,false) enabled,COALESCE(s.revision,0) revision,
    COALESCE(b.pending_minor,0) pending_minor,COALESCE(b.available_minor,0) available_minor,COALESCE(b.credited_minor,0) credited_minor
    FROM users u LEFT JOIN ad_publisher_settings s ON s.user_id=u.id
    LEFT JOIN finance_ad_publisher_balances b ON b.user_id=u.id WHERE u.id=$1")
    .bind(a.user_id).fetch_one(&mut *tx).await.map_err(db)?;
        let balance = PublisherBalance {
            enabled: row.try_get("enabled").map_err(db)?,
            revision: row.try_get("revision").map_err(db)?,
            pending_minor: row.try_get("pending_minor").map_err(db)?,
            available_minor: row.try_get("available_minor").map_err(db)?,
            credited_minor: row.try_get("credited_minor").map_err(db)?,
        };
        let rows = sqlx::query(
            "SELECT id,amount_minor,status,to_char(clear_at AT TIME ZONE 'UTC','YYYY-MM-DD')||'T'||to_char(clear_at AT TIME ZONE 'UTC','HH24:MI:SS.US')||'Z' AS clear_at FROM finance_ad_publisher_credits
    WHERE publisher_id=$1 AND ($2::BIGINT IS NULL OR id<$2) ORDER BY id DESC LIMIT 100",
        )
        .bind(a.user_id)
        .bind(before)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        let credits = rows
            .iter()
            .map(|r| {
                Ok(PublisherCredit {
                    id: r.try_get("id").map_err(db)?,
                    amount_minor: r.try_get("amount_minor").map_err(db)?,
                    status: r.try_get("status").map_err(db)?,
                    clear_at: r.try_get("clear_at").map_err(db)?,
                })
            })
            .collect::<Result<_, AdError>>()?;
        tx.commit().await.map_err(db)?;
        Ok((balance, credits))
    }
    pub async fn publisher_settings(
        &self,
        a: &AdActor,
        expected: i64,
        enabled: bool,
    ) -> Result<(), AdError> {
        if expected < 0 {
            return Err(AdError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        actor(&mut tx, a, false).await?;
        let updated = if expected == 0 {
            sqlx::query("INSERT INTO ad_publisher_settings(user_id,enabled) VALUES($1,$2) ON CONFLICT(user_id) DO NOTHING")
    .bind(a.user_id).bind(enabled).execute(&mut *tx).await.map_err(db)?.rows_affected()
        } else {
            sqlx::query("UPDATE ad_publisher_settings SET enabled=$2,revision=revision+1,updated_at=clock_timestamp()
    WHERE user_id=$1 AND revision=$3").bind(a.user_id).bind(enabled).bind(expected).execute(&mut *tx).await.map_err(db)?.rows_affected()
        };
        if updated != 1 {
            return Err(AdError::Conflict);
        }
        sqlx::query("INSERT INTO ad_planner_control(singleton,dirty) VALUES(true,true) ON CONFLICT(singleton) DO UPDATE SET dirty=true")
    .execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }
}
fn campaign(r: &sqlx::postgres::PgRow) -> Result<Campaign, AdError> {
    Ok(Campaign {
        id: r.try_get("id").map_err(db)?,
        book_id: r.try_get("book_id").map_err(db)?,
        title: r.try_get("title").map_err(db)?,
        product: r.try_get("product").map_err(db)?,
        status: r.try_get("status").map_err(db)?,
        revision: r.try_get("revision").map_err(db)?,
        budget_minor: r.try_get("budget_minor").map_err(db)?,
        target: r.try_get("target_units").map_err(db)?,
        reserved: r.try_get("reserved_units").map_err(db)?,
        reserved_minor: r.try_get("reserved_minor").map_err(db)?,
        settled_minor: r.try_get("settled_minor").map_err(db)?,
        refunded_minor: r.try_get("refunded_minor").map_err(db)?,
        starts_at: r.try_get("starts_at").map_err(db)?,
        ends_at: r.try_get("ends_at").map_err(db)?,
        clicks: r.try_get("clicks").map_err(db)?,
        opens: r.try_get("opens").map_err(db)?,
        reads: r.try_get("reads").map_err(db)?,
        library_adds: r.try_get("library_adds").map_err(db)?,
        purchases: r.try_get("purchases").map_err(db)?,
    })
}

pub(super) async fn load_daily(
    tx: &mut Transaction<'_, Postgres>,
    all: bool,
) -> Result<Vec<DailyOffer>, AdError> {
    let rows = sqlx::query(
            "SELECT o.id,o.enabled,r.* FROM ad_daily_offers o JOIN ad_daily_offer_revisions r
   ON r.offer_id=o.id AND r.revision=o.current_revision WHERE (o.enabled OR $1) ORDER BY o.id LIMIT 100",
        )
        .bind(all).fetch_all(&mut **tx)
        .await
        .map_err(db)?;
    rows.iter()
        .map(|r| {
            Ok(DailyOffer {
                id: r.try_get("id").map_err(db)?,
                enabled: r.try_get("enabled").map_err(db)?,
                revision: r.try_get("revision").map_err(db)?,
                slot: r.try_get("slot").map_err(db)?,
                rate_minor: r.try_get("rate_minor").map_err(db)?,
                min_days: r.try_get("min_days").map_err(db)?,
                max_days: r.try_get("max_days").map_err(db)?,
                terms_version: r.try_get("terms_version").map_err(db)?,
                cancellation_policy: r.try_get("cancellation_policy").map_err(db)?,
            })
        })
        .collect()
}

pub(super) async fn load_packages(
    tx: &mut Transaction<'_, Postgres>,
    all: bool,
) -> Result<Vec<ChapterPackage>, AdError> {
    let rows = sqlx::query(
            "SELECT p.id,p.enabled,r.* FROM ad_packages p JOIN ad_package_revisions r
   ON r.package_id=p.id AND r.revision=p.current_revision WHERE (p.enabled OR $1) ORDER BY p.id LIMIT 100",
        )
        .bind(all).fetch_all(&mut **tx)
        .await
        .map_err(db)?;
    rows.iter()
        .map(|r| {
            Ok(ChapterPackage {
                id: r.try_get("id").map_err(db)?,
                enabled: r.try_get("enabled").map_err(db)?,
                revision: r.try_get("revision").map_err(db)?,
                price_minor: r.try_get("price_minor").map_err(db)?,
                target_impressions: r.try_get("target_impressions").map_err(db)?,
                publisher_share_bps: r.try_get("publisher_share_bps").map_err(db)?,
                hold_seconds: r.try_get("hold_seconds").map_err(db)?,
                daily_cap: r.try_get("daily_cap").map_err(db)?,
                hourly_cap: r.try_get("hourly_cap").map_err(db)?,
                session_cap: r.try_get("session_cap").map_err(db)?,
                terms_version: r.try_get("terms_version").map_err(db)?,
            })
        })
        .collect()
}
