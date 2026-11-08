//! B/S065: read-only, unsealed chapter bodies for a separately authorized
//! site administrator. Actor/session checks belong to D; this never returns
//! sealed or S3-attached paid bytes and is never mounted on public routes.
use sqlx::PgPool;
use tor_contracts::paid_delivery::{ChapterVersionRef, DeliveryError};

#[derive(Clone)]
pub struct PgAdminUnsealedText {
    pool: PgPool,
}

impl PgAdminUnsealedText {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn read(&self, pinned: ChapterVersionRef) -> Result<Option<String>, DeliveryError> {
        let text: Option<String> = sqlx::query_scalar(
            "SELECT v.body_markdown
             FROM chapter_versions v
             JOIN chapters c ON c.id=v.chapter_id
             JOIN books b ON b.id=c.book_id
             WHERE v.chapter_id=$1 AND v.id=$2
               AND v.storage_object_id IS NULL
               AND length(v.body_markdown) BETWEEN 1 AND 200000",
        )
        .bind(pinned.chapter_id.get())
        .bind(pinned.version_id.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeliveryError::Unavailable)?;
        Ok(text.filter(|body| body.len() <= 800_000 && !body.contains('\0')))
    }
}
