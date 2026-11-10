use super::{books, unavailable, PgDiscoveryAudience};
use crate::services::audience::{
    groups::{
        GroupDiscoveryQuery, GroupDiscoveryReport, GroupDiscoveryRow, GROUP_DISCOVERY_PAGE_SIZE,
    },
    DiscoveryError, DiscoveryHealth,
};
use sqlx::{postgres::PgRow, Row};

mod sql;

impl PgDiscoveryAudience {
    /// Both periods, current eligibility, all totals and the page share one snapshot.
    /// Authorization belongs to the caller; this method never rebuilds aggregates.
    pub async fn groups_report(
        &self,
        query: &GroupDiscoveryQuery,
    ) -> Result<GroupDiscoveryReport, DiscoveryError> {
        let query = GroupDiscoveryQuery::new(query.days, query.kind, query.after.clone())?;
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
            .bind(None::<&str>)
            .bind(None::<&str>)
            .bind(false)
            .bind(false)
            .bind(query.kind.as_str())
            .bind(query.after.as_deref())
            .bind((GROUP_DISCOVERY_PAGE_SIZE + 1) as i64)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        let report = decode(&query, &rows)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(report)
    }
}

fn decode(
    query: &GroupDiscoveryQuery,
    rows: &[PgRow],
) -> Result<GroupDiscoveryReport, DiscoveryError> {
    if rows.len() < 2 || rows.len() > 2 + (GROUP_DISCOVERY_PAGE_SIZE + 1) * 2 {
        return Err(DiscoveryError::Unavailable);
    }
    let current = books::period(&rows[0], 0)?;
    let previous = books::period(&rows[1], 1)?;
    let mut groups = Vec::new();
    for pair in rows[2..].chunks(2) {
        if pair.len() != 2 {
            return Err(DiscoveryError::Unavailable);
        }
        let label: Option<String> = pair[0].try_get("label").map_err(unavailable)?;
        let previous_label: Option<String> = pair[1].try_get("label").map_err(unavailable)?;
        let matched_books: i64 = pair[0].try_get("matched_books").map_err(unavailable)?;
        if label != previous_label
            || matched_books
                != pair[1]
                    .try_get::<i64, _>("matched_books")
                    .map_err(unavailable)?
            || pair[0].try_get::<i32, _>("ordinal").map_err(unavailable)? != 0
            || pair[1].try_get::<i32, _>("ordinal").map_err(unavailable)? != 1
        {
            return Err(DiscoveryError::Unavailable);
        }
        groups.push(GroupDiscoveryRow {
            label,
            matched_books,
            current: books::metrics(&pair[0])?,
            previous: books::metrics(&pair[1])?,
            current_counts: books::counts(&pair[0])?,
            previous_counts: books::counts(&pair[1])?,
        });
    }
    let next_after = if groups.len() > GROUP_DISCOVERY_PAGE_SIZE {
        groups.truncate(GROUP_DISCOVERY_PAGE_SIZE);
        // Missing sorts first, so a full page always ends with a named group.
        Some(
            groups
                .last()
                .and_then(|row| row.label.clone())
                .ok_or(DiscoveryError::Unavailable)?,
        )
    } else {
        None
    };
    Ok(GroupDiscoveryReport {
        days: query.days,
        kind: query.kind,
        measured_at_utc: rows[0].try_get("measured_at").map_err(unavailable)?,
        current,
        previous,
        health: DiscoveryHealth {
            last_success_utc: rows[0].try_get("last_success").map_err(unavailable)?,
            fresh: rows[0].try_get("fresh").map_err(unavailable)?,
            rec_dirty: rows[0].try_get("rec_dirty").map_err(unavailable)?,
            organic_dirty: rows[0].try_get("organic_dirty").map_err(unavailable)?,
        },
        rows: groups,
        next_after,
        matched_groups: rows[0].try_get("matched_groups").map_err(unavailable)?,
        matched_books: rows[0].try_get("matched_books").map_err(unavailable)?,
    })
}
