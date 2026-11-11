use super::{limits, PgEditorial};
use crate::domain::editorial::*;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};

pub(super) async fn apply(
    store: &PgEditorial,
    admin: &Administrator,
    key: &str,
    command: &NormalizedCommand,
) -> Result<Receipt, EditorialError> {
    if admin.user_id <= 0 || !valid_command_key(key) {
        return Err(EditorialError::Invalid);
    }
    let payload = serde_json::to_vec(command).map_err(|_| EditorialError::Invalid)?;
    let hash: [u8; 32] = Sha256::digest(payload).into();
    let key_hash: [u8; 32] = Sha256::digest(key.as_bytes()).into();
    let mut tx = store.pool.begin().await?;
    limits(&mut tx, false).await?;
    sqlx::query("SELECT set_config('tor.editorial_session',encode($1::bytea,'hex'),true)")
        .bind(admin.session_digest.as_slice())
        .execute(&mut *tx)
        .await?;
    let actor: i64 = sqlx::query_scalar("SELECT public.editorial_administrator()")
        .fetch_one(&mut *tx)
        .await?;
    if actor != admin.user_id {
        return Err(EditorialError::Denied);
    }
    // Same administrator+key serializes retries even before the first receipt exists.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68653))")
        .bind(format!("{actor}:{key}"))
        .execute(&mut *tx)
        .await?;
    if let Some(row)=sqlx::query("SELECT payload_hash,campaign_id,revision FROM editorial_commands WHERE actor_id=$1 AND command_key=$2")
        .bind(actor).bind(key_hash.as_slice()).fetch_optional(&mut *tx).await? {
        if row.try_get::<Vec<u8>,_>("payload_hash")?!=hash { return Err(EditorialError::Conflict); }
        let receipt=Receipt { campaign_id:row.try_get("campaign_id")?,revision:row.try_get("revision")? };
        tx.commit().await?;
        return Ok(receipt);
    }
    let receipt = if command.action == Action::Create {
        let spec = command.details.as_ref().ok_or(EditorialError::Invalid)?;
        eligible(&mut tx, spec.book_id).await?;
        let id=sqlx::query_scalar("INSERT INTO editorial_campaigns(book_id,slot,starts_at,ends_at,priority,created_by)
          VALUES($1,$2,to_timestamp($3::bigint::double precision),to_timestamp($4::bigint::double precision),$5,$6) RETURNING id")
            .bind(spec.book_id).bind(spec.slot.as_str()).bind(spec.starts_at).bind(spec.ends_at)
            .bind(spec.priority).bind(actor).fetch_one(&mut *tx).await?;
        Receipt {
            campaign_id: id,
            revision: 1,
        }
    } else {
        update(&mut tx, command).await?
    };
    sqlx::query("INSERT INTO editorial_commands(actor_id,command_key,payload_hash,campaign_id,revision) VALUES($1,$2,$3,$4,$5)")
        .bind(actor).bind(key_hash.as_slice()).bind(hash.as_slice()).bind(receipt.campaign_id).bind(receipt.revision).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO editorial_audit(actor_id,campaign_id,revision,action,payload_hash) VALUES($1,$2,$3,$4,$5)")
        .bind(actor).bind(receipt.campaign_id).bind(receipt.revision).bind(command.action.as_str()).bind(hash.as_slice()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(receipt)
}

async fn eligible(tx: &mut Transaction<'_, Postgres>, book: i64) -> Result<(), EditorialError> {
    // Lock B before checking its current live projection. No paid-chapter gate.
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM books WHERE id=$1 FOR SHARE")
        .bind(book)
        .fetch_optional(&mut **tx)
        .await?;
    if exists.is_none() {
        return Err(EditorialError::NotFound);
    }
    let visible: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rec_materializable_books WHERE id=$1)")
            .bind(book)
            .fetch_one(&mut **tx)
            .await?;
    if !visible {
        return Err(EditorialError::NotFound);
    }
    Ok(())
}

async fn update(
    tx: &mut Transaction<'_, Postgres>,
    command: &NormalizedCommand,
) -> Result<Receipt, EditorialError> {
    let id = command.campaign_id.ok_or(EditorialError::Invalid)?;
    let row=sqlx::query("SELECT book_id,slot,status,revision,ends_at>clock_timestamp() AS has_time FROM editorial_campaigns WHERE id=$1 FOR UPDATE")
        .bind(id).fetch_optional(&mut **tx).await?.ok_or(EditorialError::NotFound)?;
    let revision: i64 = row.try_get("revision")?;
    let status: String = row.try_get("status")?;
    let book: i64 = row.try_get("book_id")?;
    if revision != command.expected_revision {
        return Err(EditorialError::Conflict);
    }
    if status == "ended" {
        return Err(EditorialError::Conflict);
    }
    match command.action {
        Action::Update => {
            if status == "active" {
                return Err(EditorialError::Conflict);
            }
            let spec = command.details.as_ref().ok_or(EditorialError::Invalid)?;
            if spec.book_id != book || spec.slot.as_str() != row.try_get::<String, _>("slot")? {
                return Err(EditorialError::Conflict);
            }
            eligible(tx, book).await?;
            sqlx::query("UPDATE editorial_campaigns SET starts_at=to_timestamp($2::bigint::double precision),ends_at=to_timestamp($3::bigint::double precision),priority=$4,revision=revision+1,updated_at=clock_timestamp() WHERE id=$1")
                .bind(id).bind(spec.starts_at).bind(spec.ends_at).bind(spec.priority).execute(&mut **tx).await?;
        }
        action => {
            let new_status = match action {
                Action::Activate
                    if matches!(status.as_str(), "draft" | "paused")
                        && row.try_get::<bool, _>("has_time")? =>
                {
                    eligible(tx, book).await?;
                    "active"
                }
                Action::Pause if status == "active" => "paused",
                Action::End => "ended",
                _ => return Err(EditorialError::Conflict),
            };
            sqlx::query("UPDATE editorial_campaigns SET status=$2,revision=revision+1,updated_at=clock_timestamp() WHERE id=$1")
                .bind(id).bind(new_status).execute(&mut **tx).await?;
        }
    }
    Ok(Receipt {
        campaign_id: id,
        revision: revision + 1,
    })
}
