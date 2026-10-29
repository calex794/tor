//! Explicit post-completion demand for more chapters. Neither a Follow nor an entitlement.
#![forbid(unsafe_code)]
use sqlx::PgPool;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DemandState {
    NotEligible,
    Ready,
    Recorded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DemandError {
    Invalid,
    NotEligible,
    Unavailable,
}

#[derive(Clone)]
pub struct PgContinuationDemand {
    pool: PgPool,
}

impl PgContinuationDemand {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn eligible(&self, actor: i64, book: i64, chapter: i64) -> Result<bool, DemandError> {
        if actor <= 0 || book <= 0 || chapter <= 0 {
            return Err(DemandError::Invalid);
        }
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM reader_chapter_completions AS finished\n JOIN chapters AS chapter ON chapter.id=finished.chapter_id\n JOIN books AS book ON book.id=chapter.book_id\n JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id\n WHERE finished.user_id=$1 AND finished.book_id=$2\n   AND finished.chapter_id=$3 AND chapter.book_id=$2\n   AND book.visibility='public' AND NOT book.admin_hold AND NOT book.is_deleted\n   AND chapter.visibility='public' AND chapter.access_level='free'\n   AND NOT chapter.admin_hold AND NOT chapter.is_deleted\n   AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at<=now())\n   AND NOT EXISTS (\n     SELECT 1 FROM chapters AS later\n     JOIN chapter_publications AS release ON release.chapter_id=later.id\n     WHERE later.book_id=chapter.book_id AND later.chapter_no>chapter.chapter_no\n       AND later.visibility='public' AND NOT later.admin_hold AND NOT later.is_deleted\n       AND (later.reader_released_at IS NULL OR later.reader_released_at<=now())\n   ))")
            .bind(actor).bind(book).bind(chapter).fetch_one(&self.pool).await
            .map_err(|_| DemandError::Unavailable)
    }

    pub async fn state(
        &self,
        actor: i64,
        book: i64,
        chapter: i64,
    ) -> Result<DemandState, DemandError> {
        if !self.eligible(actor, book, chapter).await? {
            return Ok(DemandState::NotEligible);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM reader_continuation_demand WHERE user_id=$1 AND book_id=$2 AND frontier_chapter_id=$3)"
        ).bind(actor).bind(book).bind(chapter).fetch_one(&self.pool).await
         .map_err(|_| DemandError::Unavailable)?;
        Ok(if exists {
            DemandState::Recorded
        } else {
            DemandState::Ready
        })
    }

    pub async fn record_verified_continuation(&self, actor: i64, book: i64, chapter: i64) -> Result<(), DemandError> {
        if actor <= 0 || book <= 0 || chapter <= 0 {
            return Err(DemandError::Invalid);
        }
        // Eligibility is rechecked in the single INSERT statement, not trusted from GET.
        let added: Option<i64> = sqlx::query_scalar("INSERT INTO reader_continuation_demand(user_id,book_id,frontier_chapter_id)\n SELECT $1, chapter.book_id, chapter.id\n FROM reader_chapter_completions AS finished\n JOIN chapters AS chapter ON chapter.id=finished.chapter_id\n JOIN books AS book ON book.id=chapter.book_id\n JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id\n WHERE finished.user_id=$1 AND finished.book_id=$2 AND finished.chapter_id=$3\n   AND chapter.book_id=$2 AND book.visibility='public' AND NOT book.admin_hold\n   AND NOT book.is_deleted AND chapter.visibility='public'\n   AND chapter.access_level='free' AND NOT chapter.admin_hold AND NOT chapter.is_deleted\n   AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at<=now())\n   AND NOT EXISTS (\n     SELECT 1 FROM chapters AS later JOIN chapter_publications AS release ON release.chapter_id=later.id\n     WHERE later.book_id=chapter.book_id AND later.chapter_no>chapter.chapter_no\n       AND later.visibility='public' AND NOT later.admin_hold AND NOT later.is_deleted\n       AND (later.reader_released_at IS NULL OR later.reader_released_at<=now())\n   )\n ON CONFLICT (user_id,book_id,frontier_chapter_id) DO NOTHING RETURNING id")
            .bind(actor).bind(book).bind(chapter).fetch_optional(&self.pool).await
            .map_err(|_| DemandError::Unavailable)?;
        if added.is_some() {
            return Ok(());
        }
        match self.state(actor, book, chapter).await? {
            DemandState::Recorded => Ok(()),
            DemandState::NotEligible | DemandState::Ready => Err(DemandError::NotEligible),
        }
    }

    /// Only an already authorized book owner may request this aggregate.
    /// Readers' identifiers are never returned.
    pub async fn current_count(&self, book: i64) -> Result<i64, DemandError> {
        if book <= 0 {
            return Err(DemandError::Invalid);
        }
        sqlx::query_scalar(
            "SELECT count(*) FROM reader_continuation_demand AS demand
             WHERE demand.book_id=$1 AND demand.frontier_chapter_id=(
               SELECT chapter.id FROM chapters AS chapter
               JOIN books AS book ON book.id=chapter.book_id
               JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id
               WHERE chapter.book_id=$1 AND book.visibility='public' AND NOT book.admin_hold
                 AND NOT book.is_deleted AND chapter.visibility='public'
                 AND NOT chapter.admin_hold AND NOT chapter.is_deleted
                 AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at<=now())
               ORDER BY chapter.chapter_no DESC, chapter.id DESC LIMIT 1
             )",
        )
        .bind(book)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| DemandError::Unavailable)
    }
}
