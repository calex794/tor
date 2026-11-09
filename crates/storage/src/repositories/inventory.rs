//! G-09 / #748: operator-only, read-only S3 inventory with a durable PG cursor.
//! The writes here concern observations only, NEVER stored content or rights.
use async_trait::async_trait;
use sqlx::{PgPool, Row};

use crate::{
    domain::provider::valid_private_bucket,
    services::inventory::{
        InventoryCheckpoint, InventoryError, InventoryLease, InventoryOutcome, InventoryPage,
        InventoryPrefix,
    },
};

#[derive(Clone)]
pub struct PgStorageInventory {
    pool: PgPool,
    bucket: String,
}

impl PgStorageInventory {
    pub fn new(pool: PgPool, bucket: String) -> Result<Self, InventoryError> {
        if !valid_private_bucket(&bucket) {
            return Err(InventoryError::Invalid);
        }
        Ok(Self { pool, bucket })
    }
}

#[async_trait]
impl InventoryCheckpoint for PgStorageInventory {
    async fn claim(
        &self,
        prefix: InventoryPrefix,
    ) -> Result<Option<InventoryLease>, InventoryError> {
        sqlx::query(
            "INSERT INTO storage_inventory_scans(bucket,prefix)
             VALUES ($1,$2) ON CONFLICT (bucket,prefix) DO NOTHING",
        )
        .bind(&self.bucket)
        .bind(prefix.as_str())
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        // Atomic exclusive claim, renewed epoch protects against stale workers.
        let row = sqlx::query(
            "UPDATE storage_inventory_scans
                SET lease_epoch=lease_epoch+1,
                    lease_until=now()+INTERVAL '120 seconds',
                    updated_at=now()
              WHERE bucket=$1 AND prefix=$2
                AND (lease_until IS NULL OR lease_until <= now())
              RETURNING lease_epoch,cycle,continuation_token",
        )
        .bind(&self.bucket)
        .bind(prefix.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        row.map(|r| {
            Ok(InventoryLease {
                prefix,
                epoch: r
                    .try_get("lease_epoch")
                    .map_err(|_| InventoryError::Unavailable)?,
                cycle: r.try_get("cycle").map_err(|_| InventoryError::Unavailable)?,
                continuation_token: r
                    .try_get("continuation_token")
                    .map_err(|_| InventoryError::Unavailable)?,
            })
        })
        .transpose()
    }

    async fn commit(
        &self,
        lease: &InventoryLease,
        page: &InventoryPage,
    ) -> Result<InventoryOutcome, InventoryError> {
        page.validate(lease)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        // Lock scan state before reading the registry: only our live lease may
        // record observations, including after a delayed or crashed worker.
        let locked = sqlx::query(
            "SELECT 1 FROM storage_inventory_scans
              WHERE bucket=$1 AND prefix=$2
                AND lease_epoch=$3 AND cycle=$4 AND
                    lease_until > now() AND
                    continuation_token IS NOT DISTINCT FROM $5
              FOR UPDATE",
        )
        .bind(&self.bucket)
        .bind(lease.prefix.as_str())
        .bind(lease.epoch)
        .bind(lease.cycle)
        .bind(lease.continuation_token.as_deref())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        if locked.is_none() {
            return Err(InventoryError::LeaseLost);
        }

        // Every canonical B/D association goes through storage_objects.
        // This is observation, not enough evidence to DELETE: a fresh write
        // or a concurrent registration can still be in flight.
        let unknown: Vec<String> = sqlx::query_scalar(
            "SELECT key FROM unnest($2::text[]) AS candidate(key)
              WHERE NOT EXISTS (
                SELECT 1 FROM storage_objects AS s
                WHERE s.bucket=$1 AND s.object_key=candidate.key
              )",
        )
        .bind(&self.bucket)
        .bind(&page.keys)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| InventoryError::Unavailable)?;

        if !unknown.is_empty() {
            sqlx::query(
                "INSERT INTO storage_inventory_candidates
                      (bucket,prefix,object_key,last_cycle)
                 SELECT $1,$2,key,$3 FROM unnest($4::text[]) AS candidate(key)
                 ON CONFLICT (bucket,prefix,object_key)
                 DO UPDATE SET last_seen_at=now(),
                               last_cycle=EXCLUDED.last_cycle,
                               observations=storage_inventory_candidates.observations+1",
            )
            .bind(&self.bucket)
            .bind(lease.prefix.as_str())
            .bind(lease.cycle)
            .bind(&unknown)
            .execute(&mut *tx)
            .await
            .map_err(|_| InventoryError::Unavailable)?;
        }

        let completed = !page.truncated;
        sqlx::query(
            "UPDATE storage_inventory_scans
                SET continuation_token=$3,
                    cycle=cycle+CASE WHEN $4 THEN 1 ELSE 0 END,
                    scanned_count=scanned_count+$5,
                    registered_count=registered_count+$6,
                    unknown_count=unknown_count+$7,
                    lease_until=NULL,
                    completed_at=CASE WHEN $4 THEN now() ELSE completed_at END,
                    updated_at=now()
              WHERE bucket=$1 AND prefix=$2",
        )
        .bind(&self.bucket)
        .bind(lease.prefix.as_str())
        .bind(page.next_token.as_deref())
        .bind(completed)
        .bind(page.keys.len() as i64)
        .bind((page.keys.len() - unknown.len()) as i64)
        .bind(unknown.len() as i64)
        .execute(&mut *tx)
        .await
        .map_err(|_| InventoryError::Unavailable)?;
        tx.commit().await.map_err(|_| InventoryError::Unavailable)?;
        Ok(InventoryOutcome {
            scanned: page.keys.len(),
            unknown: unknown.len(),
            complete: completed,
        })
    }
}
