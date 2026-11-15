use super::*;
use crate::domain::money::IdempotencyKey;
use sqlx::Row;

impl PgAdPosting {
    pub(super) async fn cancel_once(
        &self,
        command: &AdCancelCommand,
    ) -> Result<AdCancellation, AdError> {
        IdempotencyKey::parse(&command.command_key).map_err(|_| AdError::Invalid)?;
        if command.reason.trim().is_empty()
            || command.reason.chars().count() > 500
            || command.expected_revision <= 0
        {
            return Err(AdError::Invalid);
        }
        let intent = self.freeze(command).await?;
        self.finish_cancel(command, intent).await
    }
    pub(super) async fn finish_cancel(
        &self,
        command: &AdCancelCommand,
        intent: i64,
    ) -> Result<AdCancellation, AdError> {
        if command.product == AdProduct::Chapter {
            self.settle(command.campaign_id, 100).await?;
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let result=async {
            let f=sqlx::query("SELECT * FROM finance_ad_funding WHERE intent_id=$1 FOR NO KEY UPDATE")
                .bind(intent).fetch_one(&mut *tx).await.map_err(db)?;
            let bound:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM finance_ad_cancel_intents
                WHERE intent_id=$1 AND actor_id=$2 AND command_key=$3 AND expected_revision=$4 AND reason=$5)")
                .bind(intent).bind(command.actor.user_id).bind(&command.command_key).bind(command.expected_revision).bind(&command.reason)
                .fetch_one(&mut *tx).await.map_err(db)?;
            if !bound {return Err(AdError::Conflict);}
            if f.try_get::<String,_>("status").map_err(db)?=="cancelled" {
                return Ok(AdCancellation {campaign_id:command.campaign_id,
                    refunded:money(f.try_get("refunded_minor").map_err(db)?)?,replayed:true,pending:false});
            }
            let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ad_impression_reservations
                WHERE intent_id=$1 AND settled_at IS NULL)").bind(intent).fetch_one(&mut *tx).await.map_err(db)?;
            if pending {return Ok(AdCancellation{campaign_id:command.campaign_id,refunded:money(0)?,replayed:false,pending:true});}
            if command.product==AdProduct::Daily {settlement::daily(&mut tx,intent,true).await?;}
            let refund:i64=sqlx::query_scalar("SELECT budget_minor-settled_minor FROM finance_ad_funding WHERE intent_id=$1")
                .bind(intent).fetch_one(&mut *tx).await.map_err(db)?;
            let owner:i64=f.try_get("advertiser_id").map_err(db)?;
            let wallet:Option<i64>=sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts
                WHERE user_id=$1 AND currency='RUB' FOR UPDATE").bind(owner).fetch_optional(&mut *tx).await.map_err(db)?;
            if wallet.is_none() {return Err(AdError::Unavailable);}
            sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE").bind(owner)
                .fetch_one(&mut *tx).await.map_err(db)?;
            let reference=format!("ads:refund:{intent}");
            let wallet_entry=if refund>0 {
                let after:i64=sqlx::query_scalar("UPDATE reader_wallet_accounts SET balance_minor=balance_minor+$2
                    WHERE user_id=$1 RETURNING balance_minor").bind(owner).bind(refund).fetch_one(&mut *tx).await.map_err(db)?;
                Some(sqlx::query_scalar("INSERT INTO reader_wallet_entries(user_id,kind,delta_minor,balance_after_minor,reference)
                    VALUES($1,'ad_refund_credit',$2,$3,$4) RETURNING id")
                    .bind(owner).bind(refund).bind(after).bind(&reference).fetch_one(&mut *tx).await.map_err(db)?)
            } else {None};
            let posting=transaction(&mut tx,intent,&reference,"refund",refund,wallet_entry).await?;
            entry(&mut tx,posting,"escrow",None,-refund).await?;entry(&mut tx,posting,"wallet",Some(owner),refund).await?;
            sqlx::query("UPDATE finance_ad_funding SET status='cancelled',refunded_minor=$2 WHERE intent_id=$1")
                .bind(intent).bind(refund).execute(&mut *tx).await.map_err(db)?;
            sqlx::query("INSERT INTO finance_ad_cancellations(intent_id,actor_id,command_key,expected_revision,reason,refunded_minor,transaction_id)
                SELECT intent_id,actor_id,command_key,expected_revision,reason,$2,$3 FROM finance_ad_cancel_intents WHERE intent_id=$1")
                .bind(intent).bind(refund).bind(posting).execute(&mut *tx).await.map_err(db)?;
            let update=match command.product {
                AdProduct::Daily=>"UPDATE ad_legacy_campaigns SET status='cancelled',revision=revision+1 WHERE funding_intent_id=$1 AND status='cancelling'",
                AdProduct::Chapter=>"UPDATE ad_chapter_campaigns SET status='cancelled',revision=revision+1 WHERE funding_intent_id=$1 AND status='cancelling'",
            };
            sqlx::query(update).bind(intent).execute(&mut *tx).await.map_err(db)?;
            Ok(AdCancellation{campaign_id:command.campaign_id,refunded:money(refund)?,replayed:false,pending:false})
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

    async fn freeze(&self, command: &AdCancelCommand) -> Result<i64, AdError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        limits(&mut tx).await?;
        let result=async {
            authorize(&mut tx,&command.actor).await?;
            let f=sqlx::query("SELECT intent_id,advertiser_id FROM finance_ad_funding
                WHERE product=$1 AND campaign_id=$2 FOR NO KEY UPDATE")
                .bind(command.product.as_str()).bind(command.campaign_id.get()).fetch_optional(&mut *tx).await.map_err(db)?
                .ok_or(AdError::NotFound)?;
            let allowed:bool=sqlx::query_scalar("SELECT $1=$2 OR EXISTS(SELECT 1 FROM account_roles
                WHERE user_id=$1 AND role='administrator' AND is_active)")
                .bind(command.actor.user_id).bind(f.try_get::<i64,_>("advertiser_id").map_err(db)?)
                .fetch_one(&mut *tx).await.map_err(db)?;
            if !allowed {return Err(AdError::Denied);}
            let intent:i64=f.try_get("intent_id").map_err(db)?;
            if let Some(saved)=sqlx::query("SELECT actor_id,command_key,expected_revision,reason FROM finance_ad_cancel_intents
                WHERE intent_id=$1").bind(intent).fetch_optional(&mut *tx).await.map_err(db)? {
                if saved.try_get::<i64,_>("actor_id").map_err(db)?!=command.actor.user_id
                    || saved.try_get::<String,_>("command_key").map_err(db)?!=command.command_key
                    || saved.try_get::<i64,_>("expected_revision").map_err(db)?!=command.expected_revision
                    || saved.try_get::<String,_>("reason").map_err(db)?!=command.reason {return Err(AdError::IdempotencyConflict);}
                return Ok(intent);
            }
            let update=match command.product {
                AdProduct::Daily=>"UPDATE ad_legacy_campaigns SET status='cancelling',revision=revision+1 WHERE funding_intent_id=$1 AND revision=$2 AND status IN('funded','paused','completed') RETURNING id",
                AdProduct::Chapter=>"UPDATE ad_chapter_campaigns SET status='cancelling',revision=revision+1 WHERE funding_intent_id=$1 AND revision=$2 AND status IN('funded','paused','completed') RETURNING id",
            };
            let updated:Option<i64>=sqlx::query_scalar(update).bind(intent).bind(command.expected_revision)
                .fetch_optional(&mut *tx).await.map_err(db)?;
            if updated.is_none() {return Err(AdError::Conflict);}
            sqlx::query("INSERT INTO finance_ad_cancel_intents(intent_id,actor_id,command_key,expected_revision,reason)
                VALUES($1,$2,$3,$4,$5)").bind(intent).bind(command.actor.user_id).bind(&command.command_key)
                .bind(command.expected_revision).bind(&command.reason).execute(&mut *tx).await.map_err(db)?;
            Ok(intent)
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
pub(super) fn money(value: i64) -> Result<MoneyMinor, AdError> {
    MoneyMinor::new(value).map_err(|_| AdError::Invalid)
}
