//! Book ownership and revenue transactions consume an A-issued one-time proof
//! under row locks in the SAME transaction as the Book/finance mutation.
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};

pub(crate) struct BookStepUpScope<'a> {
    pub name: &'a str,
    pub payload_digest: &'a [u8; 32],
}

pub(crate) async fn consume_book_step_up(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    session_digest: &[u8; 32],
    token: &str,
    book: i64,
    revision: i64,
    scope: BookStepUpScope<'_>,
) -> Result<bool, sqlx::Error> {
    if actor <= 0
        || book <= 0
        || revision <= 0
        || !matches!(scope.name, "book.owner.transfer.v1" | "book.revenue.shares.v1")
        || token.len() != 43
        || !token
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-' || ch == b'_')
    {
        return Ok(false);
    }
    // A password rotation locks users before sessions. Keep that order here.
    let active: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE id=$1 AND account_state='active' FOR SHARE")
            .bind(actor)
            .fetch_optional(&mut **tx)
            .await?;
    if active != Some(actor) {
        return Ok(false);
    }
    let hash = Sha256::digest(token.as_bytes());
    let valid: Option<bool> = sqlx::query_scalar(
        "SELECT COALESCE(
            step_up_proof_hash=$3
            AND step_up_scope=$4 AND step_up_resource_id=$5
            AND step_up_revision=$6 AND step_up_payload_digest=$7
            AND step_up_issued_at<=clock_timestamp()
            AND step_up_expires_at>clock_timestamp()
            AND step_up_used_at IS NULL,FALSE)
         FROM sessions WHERE user_id=$1 AND token_hash=$2
           AND revoked_at IS NULL AND expires_at>clock_timestamp()
         FOR UPDATE",
    )
    .bind(actor)
    .bind(session_digest.as_slice())
    .bind(hash.as_slice())
    .bind(scope.name)
    .bind(book)
    .bind(revision)
    .bind(scope.payload_digest.as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    if valid != Some(true) {
        return Ok(false);
    }
    let used: Option<i64> = sqlx::query_scalar(
        "UPDATE sessions SET step_up_used_at=clock_timestamp()
         WHERE user_id=$1 AND token_hash=$2 AND step_up_proof_hash=$3
           AND step_up_used_at IS NULL AND step_up_expires_at>clock_timestamp()
         RETURNING user_id",
    )
    .bind(actor)
    .bind(session_digest.as_slice())
    .bind(hash.as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    Ok(used == Some(actor))
}
