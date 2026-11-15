use super::*;
use sqlx::Row;

impl PgAdPosting {
    /// Trusted worker operation; never an HTTP funding entry point.
    pub async fn mature_due(&self, limit: u32) -> Result<u32, AdError> {
        let (count, _) = self.mature(None, limit).await?;
        Ok(count)
    }
    pub(super) async fn mature_actor(
        &self,
        actor: &AdActor,
        limit: u32,
    ) -> Result<MoneyMinor, AdError> {
        let (_, amount) = self.mature(Some(actor), limit).await?;
        cancel::money(amount)
    }
    async fn mature(&self, actor: Option<&AdActor>, limit: u32) -> Result<(u32, i64), AdError> {
        if limit == 0 || limit > 100 {
            return Err(AdError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let result=async {
            if let Some(actor)=actor {authorize(&mut tx,actor).await?;}
            let rows=sqlx::query("SELECT * FROM finance_ad_publisher_credits WHERE status='pending'
                AND clear_at<=clock_timestamp() AND ($1::BIGINT IS NULL OR publisher_id=$1)
                ORDER BY publisher_id,id LIMIT $2 FOR UPDATE SKIP LOCKED")
                .bind(actor.map(|v|v.user_id)).bind(i64::from(limit)).fetch_all(&mut *tx).await.map_err(db)?;
            let mut total=0_i64;
            for row in &rows {
                let id:i64=row.try_get("id").map_err(db)?;
                let intent:i64=row.try_get("intent_id").map_err(db)?;
                let publisher:i64=row.try_get("publisher_id").map_err(db)?;
                let amount:i64=row.try_get("amount_minor").map_err(db)?;
                let posting=transaction(&mut tx,intent,&format!("ads:mature:{id}"),"publisher_mature",amount,None).await?;
                entry(&mut tx,posting,"publisher_pending",Some(publisher),-amount).await?;
                entry(&mut tx,posting,"publisher_available",Some(publisher),amount).await?;
                sqlx::query("UPDATE finance_ad_publisher_balances SET pending_minor=pending_minor-$2,
                    available_minor=available_minor+$2 WHERE user_id=$1")
                    .bind(publisher).bind(amount).execute(&mut *tx).await.map_err(db)?;
                sqlx::query("UPDATE finance_ad_publisher_credits SET status='available',maturity_transaction_id=$2 WHERE id=$1")
                    .bind(id).bind(posting).execute(&mut *tx).await.map_err(db)?;
                total=total.checked_add(amount).ok_or(AdError::Invalid)?;
            }
            Ok((u32::try_from(rows.len()).map_err(|_|AdError::Invalid)?,total))
        }.await;
        match result {
            Ok(v) => {
                tx.commit().await.map_err(db)?;
                Ok(v)
            }
            Err(e) => {
                tx.rollback().await.map_err(db)?;
                Err(e)
            }
        }
    }
    pub(super) async fn release_once(
        &self,
        actor: &AdActor,
        key: &str,
        limit: u32,
    ) -> Result<MoneyMinor, AdError> {
        if limit == 0 || limit > 100 {
            return Err(AdError::Invalid);
        }
        crate::domain::money::IdempotencyKey::parse(key).map_err(|_| AdError::Invalid)?;
        let mut prepare = self.pool.begin().await.map_err(db)?;
        limits(&mut prepare).await?;
        authorize(&mut prepare, actor).await?;
        sqlx::query("INSERT INTO finance_ad_publisher_transfers(user_id,command_key,batch_limit) VALUES($1,$2,$3)
            ON CONFLICT(user_id,command_key) DO NOTHING").bind(actor.user_id).bind(key).bind(i64::from(limit))
            .execute(&mut *prepare).await.map_err(db)?;
        prepare.commit().await.map_err(db)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let result=async {
            authorize(&mut tx,actor).await?;
            let transfer=sqlx::query("SELECT * FROM finance_ad_publisher_transfers WHERE user_id=$1 AND command_key=$2 FOR UPDATE")
                .bind(actor.user_id).bind(key).fetch_one(&mut *tx).await.map_err(db)?;
            if i64::from(transfer.try_get::<i32,_>("batch_limit").map_err(db)?)!=i64::from(limit){return Err(AdError::IdempotencyConflict);}
            if transfer.try_get::<String,_>("status").map_err(db)?=="posted"{return cancel::money(transfer.try_get("amount_minor").map_err(db)?);}
            let transfer_id:i64=transfer.try_get("id").map_err(db)?;
            sqlx::query("INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) VALUES($1,0,'RUB')
                ON CONFLICT(user_id) DO NOTHING").bind(actor.user_id).execute(&mut *tx).await.map_err(db)?;
            let mut balance=lock_wallet(&mut tx,actor).await?;
            let rows=sqlx::query("SELECT * FROM finance_ad_publisher_credits WHERE publisher_id=$1
                AND status='available' AND clear_at<=clock_timestamp() ORDER BY id LIMIT $2 FOR UPDATE")
                .bind(actor.user_id).bind(i64::from(limit)).fetch_all(&mut *tx).await.map_err(db)?;
            let mut total=0_i64;
            for row in rows {
                let id:i64=row.try_get("id").map_err(db)?;
                let intent:i64=row.try_get("intent_id").map_err(db)?;
                let amount:i64=row.try_get("amount_minor").map_err(db)?;
                balance=balance.checked_add(amount).ok_or(AdError::Invalid)?;
                let identity=format!("ads:publisher:{id}");
                sqlx::query("UPDATE reader_wallet_accounts SET balance_minor=$2 WHERE user_id=$1")
                    .bind(actor.user_id).bind(balance).execute(&mut *tx).await.map_err(db)?;
                let wallet:i64=sqlx::query_scalar("INSERT INTO reader_wallet_entries(user_id,kind,delta_minor,balance_after_minor,reference)
                    VALUES($1,'ad_publisher_credit',$2,$3,$4) RETURNING id")
                    .bind(actor.user_id).bind(amount).bind(balance).bind(&identity).fetch_one(&mut *tx).await.map_err(db)?;
                let posting=transaction(&mut tx,intent,&identity,"publisher_credit",amount,Some(wallet)).await?;
                entry(&mut tx,posting,"publisher_available",Some(actor.user_id),-amount).await?;
                entry(&mut tx,posting,"wallet",Some(actor.user_id),amount).await?;
                sqlx::query("UPDATE finance_ad_publisher_balances SET available_minor=available_minor-$2,
                    credited_minor=credited_minor+$2 WHERE user_id=$1")
                    .bind(actor.user_id).bind(amount).execute(&mut *tx).await.map_err(db)?;
                sqlx::query("UPDATE finance_ad_publisher_credits SET status='credited',wallet_entry_id=$2,
                    credited_at=clock_timestamp(),publisher_transfer_id=$3 WHERE id=$1")
                    .bind(id).bind(wallet).bind(transfer_id).execute(&mut *tx).await.map_err(db)?;
                total=total.checked_add(amount).ok_or(AdError::Invalid)?;
            }
            sqlx::query("UPDATE finance_ad_publisher_transfers SET status='posted',amount_minor=$2 WHERE id=$1")
                .bind(transfer_id).bind(total).execute(&mut *tx).await.map_err(db)?;
            cancel::money(total)
        }.await;
        match result {
            Ok(v) => {
                tx.commit().await.map_err(db)?;
                Ok(v)
            }
            Err(e) => {
                tx.rollback().await.map_err(db)?;
                Err(e)
            }
        }
    }
}
