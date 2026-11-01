use super::postgres::unavailable;
use crate::domain::{Requester, SupportError};
use sqlx::{Postgres, Transaction};

async fn lock(tx: &mut Transaction<'_, Postgres>, key: &str) -> Result<(), SupportError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(key)
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
    Ok(())
}

fn encoded(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) async fn ensure(
    tx: &mut Transaction<'_, Postgres>,
    who: &Requester,
) -> Result<i64, SupportError> {
    let (user, guest) = match who {
        Requester::Guest(g) => (None, Some(g)),
        Requester::User { id, guest } => (Some(*id), guest.as_ref()),
    };
    // Every writer uses user -> guest -> conversation lock order.
    if let Some(user) = user {
        lock(tx, &format!("tor-support-user:{user}")).await?;
    }
    if let Some(hash) = guest {
        lock(tx, &format!("tor-support-guest:{}", encoded(hash))).await?;
    }
    let mut user_chat = None;
    if let Some(user) = user {
        let id:i64=sqlx::query_scalar("INSERT INTO support_conversations(requester_user_id) VALUES($1)
          ON CONFLICT(requester_user_id) DO UPDATE SET requester_user_id=EXCLUDED.requester_user_id RETURNING id")
            .bind(user).fetch_one(&mut **tx).await.map_err(unavailable)?;
        user_chat = Some(id);
    }
    if let Some(hash) = guest {
        let consumed: Option<(i64, i64)> = sqlx::query_as(
            "SELECT user_id,conversation_id FROM support_guest_claims WHERE guest_identity_hash=$1",
        )
        .bind(hash.as_slice())
        .fetch_optional(&mut **tx)
        .await
        .map_err(unavailable)?;
        if let Some((owner, id)) = consumed {
            return if user == Some(owner) {
                Ok(id)
            } else {
                Err(SupportError::Denied)
            };
        }
        sqlx::query(
            "DELETE FROM support_conversations WHERE guest_identity_hash=$1 AND expires_at<=now()",
        )
        .bind(hash.as_slice())
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
        let guest_chat: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM support_conversations WHERE guest_identity_hash=$1 FOR UPDATE",
        )
        .bind(hash.as_slice())
        .fetch_optional(&mut **tx)
        .await
        .map_err(unavailable)?;
        if let (Some(user), Some(target), Some(source)) = (user, user_chat, guest_chat) {
            // Merge, do not replace: a user may already have a support history.
            // The original guest sender scope preserves identities and receipts.
            sqlx::query("UPDATE support_messages SET conversation_id=$1 WHERE conversation_id=$2")
                .bind(target)
                .bind(source)
                .execute(&mut **tx)
                .await
                .map_err(unavailable)?;
            sqlx::query("INSERT INTO support_guest_claims(guest_identity_hash,user_id,conversation_id) VALUES($1,$2,$3)")
                .bind(hash.as_slice()).bind(user).bind(target).execute(&mut **tx).await.map_err(unavailable)?;
            sqlx::query("DELETE FROM support_conversations WHERE id=$1")
                .bind(source)
                .execute(&mut **tx)
                .await
                .map_err(unavailable)?;
            return Ok(target);
        }
        if let Some(user_chat) = user_chat {
            return Ok(user_chat);
        }
        if let Some(guest_chat) = guest_chat {
            return Ok(guest_chat);
        }
        let alias = format!("Гость {}", encoded(hash)[..6].to_uppercase());
        return sqlx::query_scalar("INSERT INTO support_conversations(guest_identity_hash,guest_alias,expires_at) VALUES($1,$2,now()+INTERVAL '30 days') RETURNING id")
            .bind(hash.as_slice()).bind(alias).fetch_one(&mut **tx).await.map_err(unavailable);
    }
    user_chat.ok_or(SupportError::Denied)
}
