//! S036: administrator-only, support-linked single-purchase refund preparation.
//! This creates an immutable financial intent, never moves funds. The existing
//! D-06 approval service performs the atomic reversal after password step-up.
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupportRefundError {
    Invalid,
    Forbidden,
    NotFound,
    Conflict,
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct RefundUser {
    pub id: i64,
    pub name: String,
    pub handle: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RefundPurchase {
    pub id: i64,
    pub book: String,
    pub chapter: String,
    pub amount_minor: i64,
    pub acquired: String,
    pub batch_position: Option<i16>,
    pub first_exposed: bool,
    pub revoked: bool,
    pub request_id: Option<i64>,
    pub request_state: Option<String>,
    pub request_revision: Option<i64>,
    pub support_intent: bool,
}

#[derive(Clone, Debug)]
pub struct RefundUserDetail {
    pub user: RefundUser,
    pub support_conversation_id: Option<i64>,
    pub purchases: Vec<RefundPurchase>,
}

#[derive(Clone)]
pub struct PgSupportRefunds {
    pool: PgPool,
}

impl PgSupportRefunds {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn administrator(
        tx: &mut Transaction<'_, Postgres>,
        digest: &[u8; 32],
    ) -> Result<i64, SupportRefundError> {
        let who: Option<i64> = sqlx::query_scalar(
            "SELECT u.id FROM sessions s
             JOIN users u ON u.id=s.user_id
             JOIN account_roles a ON a.user_id=u.id
             WHERE s.token_hash=$1 AND s.revoked_at IS NULL
               AND s.expires_at>clock_timestamp()
               AND u.account_state='active' AND a.role='administrator'
               AND a.is_active FOR SHARE OF s,u,a",
        )
        .bind(digest.as_slice())
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| SupportRefundError::Unavailable)?;
        who.ok_or(SupportRefundError::Forbidden)
    }

    async fn audit(
        tx: &mut Transaction<'_, Postgres>,
        actor: i64,
        action: &str,
        buyer: Option<i64>,
        count: usize,
    ) -> Result<(), SupportRefundError> {
        sqlx::query(
            "INSERT INTO finance_direct_refund_admin_access_audit
             (admin_user_id,action,before_id,result_count)
             VALUES($1,$2,$3,$4)",
        )
        .bind(actor)
        .bind(action)
        .bind(buyer)
        .bind(i32::try_from(count).map_err(|_| SupportRefundError::Invalid)?)
        .execute(&mut **tx)
        .await
        .map_err(|_| SupportRefundError::Unavailable)?;
        Ok(())
    }

    /// Search is limited; a display-name collision is NOT resolved silently.
    /// Never search email/password or expose accounts to a non-administrator.
    pub async fn search(
        &self,
        digest: &[u8; 32],
        term: &str,
    ) -> Result<Vec<RefundUser>, SupportRefundError> {
        let query = term.trim();
        if query.chars().count() > 80
            || (query.chars().count() < 2 && query.parse::<i64>().is_err())
        {
            return Err(SupportRefundError::Invalid);
        }
        let requested_id = query.parse::<i64>().ok().filter(|id| *id > 0);
        let handle = query.trim_start_matches('@');
        let mut tx = self.pool.begin().await.map_err(|_| SupportRefundError::Unavailable)?;
        let admin = Self::administrator(&mut tx, digest).await?;
        let rows = sqlx::query(
            "SELECT u.id,u.display_name,s.handle
             FROM users u LEFT JOIN account_settings s ON s.user_id=u.id
             WHERE ($2::bigint IS NOT NULL AND u.id=$2)
                OR ($2::bigint IS NULL AND (
                  strpos(lower(u.display_name),lower($1))>0
                  OR strpos(lower(COALESCE(s.handle,'')),lower($3))>0
                ))
             ORDER BY CASE WHEN lower(COALESCE(s.handle,''))=lower($3)
                           THEN 0 ELSE 1 END, u.id
             LIMIT 12",
        )
        .bind(query)
        .bind(requested_id)
        .bind(handle)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| SupportRefundError::Unavailable)?;
        let users: Vec<RefundUser> = rows
            .into_iter()
            .map(|r| {
                Ok(RefundUser {
                    id: r.try_get("id").map_err(|_| SupportRefundError::Unavailable)?,
                    name: r.try_get("display_name").map_err(|_| SupportRefundError::Unavailable)?,
                    handle: r.try_get("handle").map_err(|_| SupportRefundError::Unavailable)?,
                })
            })
            .collect::<Result<_, SupportRefundError>>()?;
        Self::audit(&mut tx, admin, "search_users", None, users.len()).await?;
        tx.commit().await.map_err(|_| SupportRefundError::Unavailable)?;
        Ok(users)
    }

    pub async fn user_purchases(
        &self,
        digest: &[u8; 32],
        user_id: i64,
        before: Option<i64>,
    ) -> Result<Option<RefundUserDetail>, SupportRefundError> {
        if user_id <= 0 || before.is_some_and(|v| v <= 0) {
            return Err(SupportRefundError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| SupportRefundError::Unavailable)?;
        let admin = Self::administrator(&mut tx, digest).await?;
        let user = sqlx::query(
            "SELECT u.id,u.display_name,s.handle,
                    (SELECT c.id FROM support_conversations c
                     WHERE c.requester_user_id=u.id
                       AND EXISTS (SELECT 1 FROM support_messages m
                                   WHERE m.conversation_id=c.id
                                     AND m.sender_kind='user')
                    ) AS support_id
             FROM users u LEFT JOIN account_settings s ON s.user_id=u.id
             WHERE u.id=$1",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SupportRefundError::Unavailable)?;
        let mut purchases = Vec::new();
        if user.is_some() {
            let rows = sqlx::query(
                "SELECT p.id,b.title AS book,c.title AS chapter,p.amount_minor,
                        to_char(p.acquired_at AT TIME ZONE 'UTC',
                                'YYYY-MM-DD HH24:MI') AS acquired,
                        bi.position AS batch_position,
                        EXISTS (SELECT 1 FROM chapter_purchase_first_exposures x
                                WHERE x.purchase_id=p.id) AS first_exposed,
                        EXISTS (SELECT 1 FROM chapter_purchase_revocations x
                                WHERE x.purchase_id=p.id) AS revoked,
                        q.id AS request_id,q.state AS request_state,
                        q.revision AS request_revision,
                        COALESCE(si.request_id IS NOT NULL,FALSE) AS support_intent
                 FROM chapter_purchase_facts p
                 JOIN chapters c ON c.id=p.chapter_id
                 JOIN books b ON b.id=c.book_id
                 LEFT JOIN chapter_batch_items bi ON bi.purchase_id=p.id
                 LEFT JOIN LATERAL (
                    SELECT id,state,revision FROM finance_direct_refund_requests
                    WHERE purchase_id=p.id ORDER BY id DESC LIMIT 1
                 ) q ON TRUE
                 LEFT JOIN finance_support_refund_intents si ON si.request_id=q.id
                 WHERE p.user_id=$1 AND ($2::bigint IS NULL OR p.id<$2)
                 ORDER BY p.id DESC LIMIT 30",
            )
            .bind(user_id)
            .bind(before)
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| SupportRefundError::Unavailable)?;
            purchases = rows
                .into_iter()
                .map(|r| {
                    Ok(RefundPurchase {
                        id: r.try_get("id").map_err(|_| SupportRefundError::Unavailable)?,
                        book: r.try_get("book").map_err(|_| SupportRefundError::Unavailable)?,
                        chapter: r.try_get("chapter").map_err(|_| SupportRefundError::Unavailable)?,
                        amount_minor: r.try_get("amount_minor").map_err(|_| SupportRefundError::Unavailable)?,
                        acquired: r.try_get("acquired").map_err(|_| SupportRefundError::Unavailable)?,
                        batch_position: r.try_get("batch_position").map_err(|_| SupportRefundError::Unavailable)?,
                        first_exposed: r.try_get("first_exposed").map_err(|_| SupportRefundError::Unavailable)?,
                        revoked: r.try_get("revoked").map_err(|_| SupportRefundError::Unavailable)?,
                        request_id: r.try_get("request_id").map_err(|_| SupportRefundError::Unavailable)?,
                        request_state: r.try_get("request_state").map_err(|_| SupportRefundError::Unavailable)?,
                        request_revision: r.try_get("request_revision").map_err(|_| SupportRefundError::Unavailable)?,
                        support_intent: r.try_get("support_intent").map_err(|_| SupportRefundError::Unavailable)?,
                    })
                })
                .collect::<Result<_, SupportRefundError>>()?;
        }
        Self::audit(&mut tx, admin, "view_purchases", Some(user_id), purchases.len()).await?;
        tx.commit().await.map_err(|_| SupportRefundError::Unavailable)?;
        let detail = user.map(|r| {
            Ok(RefundUserDetail {
                user: RefundUser {
                    id: r.try_get("id").map_err(|_| SupportRefundError::Unavailable)?,
                    name: r.try_get("display_name").map_err(|_| SupportRefundError::Unavailable)?,
                    handle: r.try_get("handle").map_err(|_| SupportRefundError::Unavailable)?,
                },
                support_conversation_id: r.try_get("support_id").map_err(|_| SupportRefundError::Unavailable)?,
                purchases,
            })
        }).transpose()?;
        Ok(detail)
    }

    /// The operator chooses a specific immutable purchase, not an arbitrary
    /// amount. Preparation records support provenance, but NEVER credits money.
    pub async fn prepare_once(
        &self,
        digest: &[u8; 32],
        buyer: i64,
        purchase_id: i64,
        conversation_id: i64,
        command_key: &str,
        reason: &str,
    ) -> Result<i64, SupportRefundError> {
        let reason = reason.trim();
        if buyer <= 0 || purchase_id <= 0 || conversation_id <= 0
            || reason.is_empty() || reason.chars().count() > 1000
            || command_key.len() != 43
            || !command_key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(SupportRefundError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| SupportRefundError::Unavailable)?;
        let admin = Self::administrator(&mut tx, digest).await?;
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT p.user_id FROM chapter_purchase_facts p
             JOIN finance_chapter_postings fp ON fp.purchase_id=p.id
             JOIN library_purchased_chapters l ON l.purchase_id=p.id
               AND l.user_id=p.user_id AND l.chapter_id=p.chapter_id
               AND l.active_version_id=p.version_id
             WHERE p.id=$1 AND p.user_id=$2 AND p.currency='RUB'
             FOR UPDATE OF p",
        )
        .bind(purchase_id)
        .bind(buyer)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SupportRefundError::Unavailable)?;
        if owner.is_none() {
            return Err(SupportRefundError::NotFound);
        }
        let linked: bool = sqlx::query_scalar(
            "SELECT EXISTS (
               SELECT 1 FROM support_conversations c
               WHERE c.id=$1 AND c.requester_user_id=$2
                 AND EXISTS(SELECT 1 FROM support_messages m
                            WHERE m.conversation_id=c.id AND m.sender_kind='user')
             )",
        )
        .bind(conversation_id)
        .bind(buyer)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| SupportRefundError::Unavailable)?;
        if !linked {
            return Err(SupportRefundError::Conflict);
        }
        let revoked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM chapter_purchase_revocations
                           WHERE purchase_id=$1)
                 OR EXISTS(SELECT 1 FROM finance_direct_refund_postings
                           WHERE purchase_id=$1)",
        )
        .bind(purchase_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| SupportRefundError::Unavailable)?;
        if revoked {
            return Err(SupportRefundError::Conflict);
        }
        let existing = sqlx::query(
            "SELECT q.id,q.reason,si.admin_user_id,si.support_conversation_id
             FROM finance_direct_refund_requests q
             LEFT JOIN finance_support_refund_intents si ON si.request_id=q.id
             WHERE q.purchase_id=$1 AND q.state IN ('pending','processing')
             FOR UPDATE OF q",
        )
        .bind(purchase_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| SupportRefundError::Unavailable)?;
        if let Some(q) = existing {
            let author: Option<i64> = q.try_get("admin_user_id").map_err(|_| SupportRefundError::Unavailable)?;
            let case: Option<i64> = q.try_get("support_conversation_id").map_err(|_| SupportRefundError::Unavailable)?;
            let old_reason: String = q.try_get("reason").map_err(|_| SupportRefundError::Unavailable)?;
            if author == Some(admin) && case == Some(conversation_id) && old_reason == reason {
                return q.try_get("id").map_err(|_| SupportRefundError::Unavailable);
            }
            return Err(SupportRefundError::Conflict);
        }
        let digest_value = Sha256::digest(format!(
            "s036.admin.support.v1:{admin}:{buyer}:{purchase_id}:{conversation_id}:{reason}"
        ).as_bytes());
        let request_id: i64 = sqlx::query_scalar(
            "INSERT INTO finance_direct_refund_requests
             (purchase_id,user_id,request_key,request_digest,reason)
             VALUES ($1,$2,$3,$4,$5) RETURNING id",
        )
        .bind(purchase_id)
        .bind(buyer)
        .bind(command_key)
        .bind(digest_value.as_slice())
        .bind(reason)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| SupportRefundError::Unavailable)?;
        sqlx::query(
            "INSERT INTO finance_support_refund_intents
             (request_id,purchase_id,buyer_user_id,admin_user_id,
              support_conversation_id)
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(request_id)
        .bind(purchase_id)
        .bind(buyer)
        .bind(admin)
        .bind(conversation_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| SupportRefundError::Unavailable)?;
        sqlx::query(
            "INSERT INTO admin_action_audit
             (actor_id,action,resource_type,resource_id,request_id,
              command_key,outcome)
             VALUES ($1,'finance.refund.prepare','chapter_purchase',$2,
                     $3,$4,'prepared')",
        )
        .bind(admin)
        .bind(purchase_id)
        .bind(request_id.to_string())
        .bind(command_key)
        .execute(&mut *tx)
        .await
        .map_err(|_| SupportRefundError::Unavailable)?;
        tx.commit().await.map_err(|_| SupportRefundError::Unavailable)?;
        Ok(request_id)
    }
}
