use super::{unavailable, PgDiscoveryAudience};
use crate::services::audience::{
    books::{
        BookDiscoveryCounts, BookDiscoveryMetrics, BookDiscoveryPeriod, BookDiscoveryQuery,
        BookDiscoveryReport, BookDiscoveryRow, BOOK_DISCOVERY_FILTER_OPTIONS,
        BOOK_DISCOVERY_PAGE_SIZE,
    },
    DiscoveryError, DiscoveryHealth,
};
use sqlx::{postgres::PgRow, Row};

pub(super) mod sql;

impl PgDiscoveryAudience {
    /// Read-only administration report. Authorization belongs to the caller.
    /// Current public eligibility, both periods and totals share one snapshot.
    pub async fn books_report(
        &self,
        query: &BookDiscoveryQuery,
    ) -> Result<BookDiscoveryReport, DiscoveryError> {
        let query = BookDiscoveryQuery::with_missing(
            query.days,
            query.genre.clone(),
            query.tag.clone(),
            query.without_genre,
            query.without_tag,
            query.after_id,
        )?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout = '750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let rows = sqlx::query(&sql::report())
            .bind(query.days)
            .bind(query.genre.as_deref())
            .bind(query.tag.as_deref())
            .bind(query.without_genre)
            .bind(query.without_tag)
            .bind(query.after_id)
            .bind((BOOK_DISCOVERY_PAGE_SIZE + 1) as i64)
            .bind((BOOK_DISCOVERY_FILTER_OPTIONS + 1) as i64)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        let report = decode(query.days, &rows)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(report)
    }
}

fn decode(days: i32, rows: &[PgRow]) -> Result<BookDiscoveryReport, DiscoveryError> {
    if rows.len() < 2 || rows.len() > 2 + (BOOK_DISCOVERY_PAGE_SIZE + 1) * 2 {
        return Err(DiscoveryError::Unavailable);
    }
    let current = period(&rows[0], 0)?;
    let previous = period(&rows[1], 1)?;
    let mut books = Vec::new();
    for pair in rows[2..].chunks(2) {
        if pair.len() != 2 {
            return Err(DiscoveryError::Unavailable);
        }
        let book_id: i64 = pair[0].try_get("book_id").map_err(unavailable)?;
        let previous_book: i64 = pair[1].try_get("book_id").map_err(unavailable)?;
        if book_id != previous_book
            || pair[0].try_get::<i32, _>("ordinal").map_err(unavailable)? != 0
            || pair[1].try_get::<i32, _>("ordinal").map_err(unavailable)? != 1
        {
            return Err(DiscoveryError::Unavailable);
        }
        books.push(BookDiscoveryRow {
            book_id,
            title: pair[0].try_get("title").map_err(unavailable)?,
            genres: pair[0].try_get("genres").map_err(unavailable)?,
            tags: pair[0].try_get("tags").map_err(unavailable)?,
            current: metrics(&pair[0])?,
            previous: metrics(&pair[1])?,
        });
    }
    let next_after_id = if books.len() > BOOK_DISCOVERY_PAGE_SIZE {
        books.truncate(BOOK_DISCOVERY_PAGE_SIZE);
        books.last().map(|book| book.book_id)
    } else {
        None
    };
    let mut genres: Vec<String> = rows[0].try_get("genre_options").map_err(unavailable)?;
    let mut tags: Vec<String> = rows[0].try_get("tag_options").map_err(unavailable)?;
    let filter_options_truncated =
        genres.len() > BOOK_DISCOVERY_FILTER_OPTIONS || tags.len() > BOOK_DISCOVERY_FILTER_OPTIONS;
    genres.truncate(BOOK_DISCOVERY_FILTER_OPTIONS);
    tags.truncate(BOOK_DISCOVERY_FILTER_OPTIONS);
    Ok(BookDiscoveryReport {
        days,
        measured_at_utc: rows[0].try_get("measured_at").map_err(unavailable)?,
        current,
        previous,
        health: DiscoveryHealth {
            last_success_utc: rows[0].try_get("last_success").map_err(unavailable)?,
            fresh: rows[0].try_get("fresh").map_err(unavailable)?,
            rec_dirty: rows[0].try_get("rec_dirty").map_err(unavailable)?,
            organic_dirty: rows[0].try_get("organic_dirty").map_err(unavailable)?,
        },
        genres,
        tags,
        filter_options_truncated,
        rows: books,
        next_after_id,
        matched_books: rows[0].try_get("matched_books").map_err(unavailable)?,
    })
}

pub(super) fn period(row: &PgRow, ordinal: i32) -> Result<BookDiscoveryPeriod, DiscoveryError> {
    if !row.try_get::<bool, _>("is_total").map_err(unavailable)?
        || row.try_get::<i32, _>("ordinal").map_err(unavailable)? != ordinal
    {
        return Err(DiscoveryError::Unavailable);
    }
    Ok(BookDiscoveryPeriod {
        from_day: row.try_get("from_day").map_err(unavailable)?,
        until_day_exclusive: row.try_get("until_day").map_err(unavailable)?,
        includes_partial_today: row.try_get("partial_today").map_err(unavailable)?,
        totals: metrics(row)?,
        counts: counts(row)?,
    })
}

pub(super) fn counts(row: &PgRow) -> Result<BookDiscoveryCounts, DiscoveryError> {
    Ok(BookDiscoveryCounts {
        unknown_evidence: row.try_get("unknown_evidence").map_err(unavailable)?,
        no_observations: row.try_get("no_observations").map_err(unavailable)?,
        observed_without_starts: row
            .try_get("observed_without_starts")
            .map_err(unavailable)?,
        with_starts: row.try_get("with_starts").map_err(unavailable)?,
    })
}

pub(super) fn metrics(row: &PgRow) -> Result<BookDiscoveryMetrics, DiscoveryError> {
    Ok(BookDiscoveryMetrics {
        evidence_invalidated: row.try_get("evidence_invalidated").map_err(unavailable)?,
        issued: row.try_get("issued").map_err(unavailable)?,
        impressions: row.try_get("impressions").map_err(unavailable)?,
        clicks: row.try_get("clicks").map_err(unavailable)?,
        book_opens: row.try_get("book_opens").map_err(unavailable)?,
        starts: row.try_get("starts").map_err(unavailable)?,
    })
}
