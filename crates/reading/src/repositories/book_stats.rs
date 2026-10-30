//! NEwBOO statistics for actual free chapter opens and already-committed facts.
//! A displayed chapter is not proof of completion or a paid entitlement.
//! All money comes from the immutable D purchase facts, never estimates.
use sqlx::{PgPool, Row};

pub const CHAPTER_PAGE_LIMIT: usize = 100;

#[derive(Clone)]
pub struct PgBookStats {
    pool: PgPool,
}

#[derive(Debug)]
pub struct ChapterStats {
    pub id: i64,
    pub number: i32,
    pub title: String,
    pub opens: i64,
    pub anonymous_opens: i64,
    pub unique_registered: i64,
    pub repeat_authenticated: i64,
    pub purchases: i64,
    pub purchase_minor: i64,
    pub completed: i64,
}

#[derive(Debug)]
pub struct DailyStats {
    pub day: String,
    pub opens: i64,
    pub unique_first: i64,
    pub purchases: i64,
    pub purchase_minor: i64,
}

#[derive(Debug)]
pub struct BookStatsReport {
    pub title: String,
    pub opens: i64,
    pub anonymous_opens: i64,
    pub authenticated_opens: i64,
    pub unique_registered: i64,
    pub repeat_authenticated: i64,
    pub purchases: i64,
    pub purchase_minor: i64,
    pub completed: i64,
    pub chapters: Vec<ChapterStats>,
    pub daily: Vec<DailyStats>,
    pub next_after: Option<String>,
}

impl PgBookStats {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Log only a *successful* public free-reader rendering.
    /// Caller identity is resolved from A's verified server session or is None.
    /// SQL rechecks visibility after any concurrent moderation action.
    pub async fn record_open(
        &self,
        chapter_id: i64,
        actor: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        if chapter_id <= 0 || actor.is_some_and(|id| id <= 0) {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        let book_id: Option<i64> = sqlx::query_scalar(
            "SELECT c.book_id FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.id=$1 AND c.visibility='public' AND c.access_level='free'
               AND c.price_amount=0 AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted",
        )
        .bind(chapter_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(book_id) = book_id else {
            return Ok(());
        };

        let first_chapter = if let Some(actor) = actor {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO book_open_readers (book_id,user_id)
                 VALUES ($1,$2) ON CONFLICT DO NOTHING RETURNING user_id",
            )
            .bind(book_id)
            .bind(actor)
            .fetch_optional(&mut *tx)
            .await?;
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO book_chapter_open_readers (chapter_id,book_id,user_id)
                 VALUES ($1,$2,$3) ON CONFLICT DO NOTHING RETURNING user_id",
            )
            .bind(chapter_id)
            .bind(book_id)
            .bind(actor)
            .fetch_optional(&mut *tx)
            .await?
            .is_some()
        } else {
            false
        };

        let authenticated: i64 = i64::from(actor.is_some());
        let repeated: i64 = i64::from(actor.is_some() && !first_chapter);
        let first: i64 = i64::from(first_chapter);
        sqlx::query(
            "INSERT INTO book_chapter_open_daily
                (day,chapter_id,book_id,opens,anonymous_opens,
                 authenticated_opens,repeat_authenticated_opens,first_authenticated_opens)
             VALUES ((now() AT TIME ZONE 'UTC')::date,$1,$2,1,$3,$4,$5,$6)
             ON CONFLICT (day,chapter_id) DO UPDATE SET
                opens=book_chapter_open_daily.opens+EXCLUDED.opens,
                anonymous_opens=book_chapter_open_daily.anonymous_opens+EXCLUDED.anonymous_opens,
                authenticated_opens=book_chapter_open_daily.authenticated_opens+EXCLUDED.authenticated_opens,
                repeat_authenticated_opens=book_chapter_open_daily.repeat_authenticated_opens
                    +EXCLUDED.repeat_authenticated_opens,
                first_authenticated_opens=book_chapter_open_daily.first_authenticated_opens
                    +EXCLUDED.first_authenticated_opens",
        )
        .bind(chapter_id)
        .bind(book_id)
        .bind(1_i64 - authenticated)
        .bind(authenticated)
        .bind(repeated)
        .bind(first)
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// Caller must have first passed the B-owned active BookRole check.
    /// This read never determines entitlement or access to chapter text.
    pub async fn report(
        &self,
        book_id: i64,
        after: Option<(i32, i64)>,
    ) -> Result<Option<BookStatsReport>, sqlx::Error> {
        if book_id <= 0 {
            return Ok(None);
        }
        let title: Option<String> = sqlx::query_scalar(
            "SELECT title FROM books WHERE id=$1 AND NOT admin_hold AND NOT is_deleted",
        )
        .bind(book_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(title) = title else { return Ok(None); };
        let totals = sqlx::query(
            "SELECT
               COALESCE((SELECT SUM(opens) FROM book_chapter_open_daily
                         WHERE book_id=$1),0)::BIGINT AS opens,
               COALESCE((SELECT SUM(anonymous_opens) FROM book_chapter_open_daily
                         WHERE book_id=$1),0)::BIGINT AS anonymous_opens,
               COALESCE((SELECT SUM(authenticated_opens) FROM book_chapter_open_daily
                         WHERE book_id=$1),0)::BIGINT AS authenticated_opens,
               COALESCE((SELECT SUM(repeat_authenticated_opens) FROM book_chapter_open_daily
                         WHERE book_id=$1),0)::BIGINT AS repeats,
               (SELECT COUNT(*) FROM book_open_readers WHERE book_id=$1) AS unique_registered,
               (SELECT COUNT(*) FROM chapter_purchase_facts p JOIN chapters c
                  ON c.id=p.chapter_id WHERE c.book_id=$1) AS purchases,
               COALESCE((SELECT SUM(p.amount_minor) FROM chapter_purchase_facts p
                  JOIN chapters c ON c.id=p.chapter_id WHERE c.book_id=$1),0)::BIGINT AS gross,
               (SELECT COUNT(*) FROM reader_chapter_completions WHERE book_id=$1) AS completed",
        )
        .bind(book_id)
        .fetch_one(&self.pool)
        .await?;

        let (after_no, after_id) = after.map_or((None, None), |(no, id)| (Some(no), Some(id)));
        let chapter_rows = sqlx::query(
            "SELECT c.id,c.chapter_no,c.title,
               COALESCE((SELECT SUM(x.opens) FROM book_chapter_open_daily x
                 WHERE x.chapter_id=c.id),0)::BIGINT AS opens,
               COALESCE((SELECT SUM(x.anonymous_opens) FROM book_chapter_open_daily x
                 WHERE x.chapter_id=c.id),0)::BIGINT AS anonymous_opens,
               COALESCE((SELECT SUM(x.repeat_authenticated_opens) FROM book_chapter_open_daily x
                 WHERE x.chapter_id=c.id),0)::BIGINT AS repeats,
               (SELECT COUNT(*) FROM book_chapter_open_readers x
                 WHERE x.chapter_id=c.id) AS unique_registered,
               (SELECT COUNT(*) FROM chapter_purchase_facts p
                 WHERE p.chapter_id=c.id) AS purchases,
               COALESCE((SELECT SUM(p.amount_minor) FROM chapter_purchase_facts p
                 WHERE p.chapter_id=c.id),0)::BIGINT AS gross,
               (SELECT COUNT(*) FROM reader_chapter_completions done
                 WHERE done.chapter_id=c.id) AS completed
             FROM chapters c
             WHERE c.book_id=$1 AND NOT c.is_deleted
               AND ($2::INTEGER IS NULL OR (c.chapter_no,c.id)>($2::INTEGER,$3::BIGINT))
             ORDER BY c.chapter_no,c.id LIMIT $4",
        )
        .bind(book_id)
        .bind(after_no)
        .bind(after_id)
        .bind(i64::try_from(CHAPTER_PAGE_LIMIT + 1).expect("fixed page limit"))
        .fetch_all(&self.pool)
        .await?;
        let has_next = chapter_rows.len() > CHAPTER_PAGE_LIMIT;
        let mut chapters = Vec::with_capacity(CHAPTER_PAGE_LIMIT.min(chapter_rows.len()));
        for row in chapter_rows.into_iter().take(CHAPTER_PAGE_LIMIT) {
            chapters.push(ChapterStats {
                id: row.try_get("id")?,
                number: row.try_get("chapter_no")?,
                title: row.try_get("title")?,
                opens: row.try_get("opens")?,
                anonymous_opens: row.try_get("anonymous_opens")?,
                unique_registered: row.try_get("unique_registered")?,
                repeat_authenticated: row.try_get("repeats")?,
                purchases: row.try_get("purchases")?,
                purchase_minor: row.try_get("gross")?,
                completed: row.try_get("completed")?,
            });
        }
        let next_after = if has_next {
            chapters.last().map(|last| format!("{}:{}", last.number, last.id))
        } else {
            None
        };

        let day_rows = sqlx::query(
            "WITH opens AS (
               SELECT day, SUM(opens)::BIGINT AS opens,
                      SUM(first_authenticated_opens)::BIGINT AS firsts
               FROM book_chapter_open_daily
               WHERE book_id=$1
                 AND day >= (now() AT TIME ZONE 'UTC')::date - 29
               GROUP BY day
             ), purchases AS (
               SELECT (p.acquired_at AT TIME ZONE 'UTC')::date AS day,
                      COUNT(*)::BIGINT AS purchases,
                      SUM(p.amount_minor)::BIGINT AS gross
               FROM chapter_purchase_facts p JOIN chapters c ON c.id=p.chapter_id
               WHERE c.book_id=$1
                 AND p.acquired_at >= (now() AT TIME ZONE 'UTC')::date - INTERVAL '29 days'
               GROUP BY 1
             )
             SELECT TO_CHAR(COALESCE(o.day,p.day),'YYYY-MM-DD') AS day,
                    COALESCE(o.opens,0)::BIGINT AS opens,
                    COALESCE(o.firsts,0)::BIGINT AS firsts,
                    COALESCE(p.purchases,0)::BIGINT AS purchases,
                    COALESCE(p.gross,0)::BIGINT AS gross
             FROM opens o FULL JOIN purchases p ON p.day=o.day
             ORDER BY COALESCE(o.day,p.day) DESC LIMIT 30",
        )
        .bind(book_id)
        .fetch_all(&self.pool)
        .await?;
        let daily = day_rows
            .into_iter()
            .map(|r| {
                Ok(DailyStats {
                    day: r.try_get("day")?,
                    opens: r.try_get("opens")?,
                    unique_first: r.try_get("firsts")?,
                    purchases: r.try_get("purchases")?,
                    purchase_minor: r.try_get("gross")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        Ok(Some(BookStatsReport {
            title,
            opens: totals.try_get("opens")?,
            anonymous_opens: totals.try_get("anonymous_opens")?,
            authenticated_opens: totals.try_get("authenticated_opens")?,
            unique_registered: totals.try_get("unique_registered")?,
            repeat_authenticated: totals.try_get("repeats")?,
            purchases: totals.try_get("purchases")?,
            purchase_minor: totals.try_get("gross")?,
            completed: totals.try_get("completed")?,
            chapters,
            daily,
            next_after,
        }))
    }
}
