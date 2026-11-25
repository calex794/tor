//! Publication/checkout lock order. Lock parent Books before any Chapter or
//! edition FK/row locks, in a separate READ COMMITTED statement.
use sqlx::{Postgres, Transaction};

pub(super) async fn lock_books(
    tx: &mut Transaction<'_, Postgres>,
    chapters: &[i64],
) -> Result<(), sqlx::Error> {
    // Missing chapters, access/price/owner state and replay semantics remain
    // the responsibility of the existing checkout statements. This is a
    // synchronization fence, not authorization or a change to the offer.
    sqlx::query_scalar::<_, i64>(
        "SELECT b.id FROM books b
         WHERE b.id IN (SELECT c.book_id FROM chapters c WHERE c.id=ANY($1))
         ORDER BY b.id FOR SHARE OF b",
    )
    .bind(chapters)
    .fetch_all(&mut **tx)
    .await?;
    Ok(())
}
