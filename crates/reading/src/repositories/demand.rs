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
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM reader_chapter_completions AS finished\n JOIN chapters AS chapter ON chapter.id=finished.chapter_id\n JOIN books AS book ON book.id=chapter.book_id\n JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id\n WHERE finished.user_id=$1 AND finished.book_id=$2\n   AND finished.chapter_id=$3 AND chapter.book_id=$2\n   AND book.visibility='public' AND book.visibility_status='active' AND book.lifecycle_status <> 'completed' AND NOT book.admin_hold AND NOT book.is_deleted\n   AND chapter.visibility='public' AND chapter.access_level='free'\n   AND NOT chapter.admin_hold AND NOT chapter.is_deleted\n   AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at<=now())\n   AND NOT EXISTS (\n     SELECT 1 FROM chapters AS later\n     JOIN chapter_publications AS release ON release.chapter_id=later.id\n     WHERE later.book_id=chapter.book_id AND later.chapter_no>chapter.chapter_no\n       AND later.visibility='public' AND NOT later.admin_hold AND NOT later.is_deleted\n       AND (later.reader_released_at IS NULL OR later.reader_released_at<=now())\n   ))")
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

    pub async fn record_verified_continuation(
        &self,
        actor: i64,
        book: i64,
        chapter: i64,
    ) -> Result<(), DemandError> {
        if actor <= 0 || book <= 0 || chapter <= 0 {
            return Err(DemandError::Invalid);
        }
        // Eligibility is rechecked in the single INSERT statement, not trusted from GET.
        let added: Option<i64> = sqlx::query_scalar("INSERT INTO reader_continuation_demand(user_id,book_id,frontier_chapter_id)\n SELECT $1, chapter.book_id, chapter.id\n FROM reader_chapter_completions AS finished\n JOIN chapters AS chapter ON chapter.id=finished.chapter_id\n JOIN books AS book ON book.id=chapter.book_id\n JOIN chapter_publications AS publication ON publication.chapter_id=chapter.id\n WHERE finished.user_id=$1 AND finished.book_id=$2 AND finished.chapter_id=$3\n   AND chapter.book_id=$2 AND book.visibility='public' AND book.visibility_status='active' AND book.lifecycle_status <> 'completed' AND NOT book.admin_hold\n   AND NOT book.is_deleted AND chapter.visibility='public'\n   AND chapter.access_level='free' AND NOT chapter.admin_hold AND NOT chapter.is_deleted\n   AND (chapter.reader_released_at IS NULL OR chapter.reader_released_at<=now())\n   AND NOT EXISTS (\n     SELECT 1 FROM chapters AS later JOIN chapter_publications AS release ON release.chapter_id=later.id\n     WHERE later.book_id=chapter.book_id AND later.chapter_no>chapter.chapter_no\n       AND later.visibility='public' AND NOT later.admin_hold AND NOT later.is_deleted\n       AND (later.reader_released_at IS NULL OR later.reader_released_at<=now())\n   )\n ON CONFLICT (user_id,book_id,frontier_chapter_id) DO NOTHING RETURNING id")
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
               WHERE chapter.book_id=$1 AND book.visibility='public' AND book.visibility_status='active' AND book.lifecycle_status <> 'completed' AND NOT book.admin_hold
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


/// A scoped, read-only aggregate. Never returns reader identifiers.
#[derive(Debug, Clone)]
pub struct ContinuationStatsBook {
    pub book_id: i64,
    pub title: String,
    pub owner_user_id: i64,
    pub owner_name: String,
    pub latest_chapter_no: Option<i32>,
    pub waiting: i64,
    pub new_seven_days: i64,
    pub historical: i64,
}

#[derive(Debug, Clone)]
pub struct ContinuationStatsReport {
    pub books: Vec<ContinuationStatsBook>,
    pub total_books: i64,
    pub waiting_books: i64,
    pub total_waiting: i64,
    pub new_seven_days: i64,
    pub historical: i64,
    pub next_after: Option<(i64, i64)>,
}

#[derive(sqlx::FromRow)]
struct ContinuationStatsRecord {
    book_id: Option<i64>,
    title: Option<String>,
    owner_user_id: Option<i64>,
    owner_name: Option<String>,
    latest_chapter_no: Option<i32>,
    waiting: Option<i64>,
    new_seven_days: Option<i64>,
    historical: Option<i64>,
    total_books: i64,
    waiting_books: i64,
    total_waiting: i64,
    total_new_seven_days: i64,
    total_historical: i64,
}

impl PgContinuationDemand {
    /// Caller must first establish ownership, or a fresh administrator role
    /// when owner_id is None. The book scope is applied inside PostgreSQL.
    pub async fn stats_report(
        &self,
        owner_id: Option<i64>,
        after: Option<(i64, i64)>,
    ) -> Result<ContinuationStatsReport, DemandError> {
        if owner_id.is_some_and(|id| id <= 0)
            || after.is_some_and(|(waiting, book)| waiting < 0 || book <= 0)
        {
            return Err(DemandError::Invalid);
        }
        // The same current frontier conditions as current_count: a newer
        // reader-visible release resets current demand, not historical facts.
        // Keyset order is (waiting DESC, book_id DESC), capped at 100 rows.
        let records: Vec<ContinuationStatsRecord> = sqlx::query_as(
            r#"WITH scoped AS MATERIALIZED (
                SELECT book.id AS book_id, book.title, book.owner_user_id,
                       owner.display_name AS owner_name,
                       frontier.chapter_no AS latest_chapter_no,
                       active.waiting, active.new_seven_days,
                       history.historical
                FROM books AS book
                JOIN users AS owner ON owner.id=book.owner_user_id
                LEFT JOIN LATERAL (
                    SELECT chapter.id, chapter.chapter_no
                    FROM chapters AS chapter
                    JOIN chapter_publications AS publication
                      ON publication.chapter_id=chapter.id
                    WHERE chapter.book_id=book.id
                      AND book.visibility='public'
                      AND book.visibility_status='active'
                      AND book.lifecycle_status<>'completed'
                      AND NOT book.admin_hold AND NOT book.is_deleted
                      AND chapter.visibility='public'
                      AND NOT chapter.admin_hold AND NOT chapter.is_deleted
                      AND (chapter.reader_released_at IS NULL
                           OR chapter.reader_released_at<=now())
                    ORDER BY chapter.chapter_no DESC, chapter.id DESC
                    LIMIT 1
                ) AS frontier ON TRUE
                LEFT JOIN LATERAL (
                    SELECT count(*) AS waiting,
                           count(*) FILTER (
                             WHERE demand.created_at>=now()-interval '7 days'
                           ) AS new_seven_days
                    FROM reader_continuation_demand AS demand
                    WHERE demand.book_id=book.id
                      AND demand.frontier_chapter_id=frontier.id
                ) AS active ON TRUE
                LEFT JOIN LATERAL (
                    SELECT count(*) AS historical
                    FROM reader_continuation_demand AS demand
                    WHERE demand.book_id=book.id
                ) AS history ON TRUE
                WHERE ($1::bigint IS NULL OR book.owner_user_id=$1::bigint)
            ), totals AS (
                SELECT count(*) AS total_books,
                       count(*) FILTER (WHERE waiting>0) AS waiting_books,
                       COALESCE(sum(waiting),0)::bigint AS total_waiting,
                       COALESCE(sum(new_seven_days),0)::bigint AS total_new_seven_days,
                       COALESCE(sum(historical),0)::bigint AS total_historical
                FROM scoped
            ), page AS (
                SELECT * FROM scoped
                WHERE ($2::bigint IS NULL
                       OR waiting<$2::bigint
                       OR (waiting=$2::bigint AND book_id<$3::bigint))
                ORDER BY waiting DESC, book_id DESC
                LIMIT 101
            )
            SELECT page.book_id, page.title, page.owner_user_id,
                   page.owner_name, page.latest_chapter_no,
                   page.waiting, page.new_seven_days, page.historical,
                   totals.total_books, totals.waiting_books,
                   totals.total_waiting, totals.total_new_seven_days,
                   totals.total_historical
            FROM totals LEFT JOIN page ON TRUE
            ORDER BY page.waiting DESC NULLS LAST, page.book_id DESC NULLS LAST"#,
        )
        .bind(owner_id)
        .bind(after.map(|key| key.0))
        .bind(after.map(|key| key.1))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| DemandError::Unavailable)?;
        let summary = records.first().ok_or(DemandError::Unavailable)?;
        let (total_books, waiting_books, total_waiting, new_seven_days, historical) = (
            summary.total_books,
            summary.waiting_books,
            summary.total_waiting,
            summary.total_new_seven_days,
            summary.total_historical,
        );
        let mut books: Vec<_> = records
            .into_iter()
            .filter_map(|row| {
                Some(ContinuationStatsBook {
                    book_id: row.book_id?,
                    title: row.title?,
                    owner_user_id: row.owner_user_id?,
                    owner_name: row.owner_name?,
                    latest_chapter_no: row.latest_chapter_no,
                    waiting: row.waiting?,
                    new_seven_days: row.new_seven_days?,
                    historical: row.historical?,
                })
            })
            .collect();
        let has_next = books.len() > 100;
        books.truncate(100);
        let next_after = if has_next {
            books.last().map(|book| (book.waiting, book.book_id))
        } else {
            None
        };
        Ok(ContinuationStatsReport {
            books,
            total_books,
            waiting_books,
            total_waiting,
            new_seven_days,
            historical,
            next_after,
        })
    }
}
