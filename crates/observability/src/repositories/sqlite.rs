//! Single-writer SQLite trace archive with bounded retention and honest search.
use crate::{
    domain::{RouteGroup, StatusGroup},
    services::{
        trace::{RequestTrace, SqlEvidence, SqlSpan},
        wire,
    },
};
use rusqlite::{params, Connection, OptionalExtension};
use std::{fmt, fs, path::Path};

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    Invalid,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(_) => write!(f, "SQLite store failed"),
            Self::Io(_) => write!(f, "trace-store file failed"),
            Self::Invalid => write!(f, "invalid trace-store value or policy"),
        }
    }
}

impl std::error::Error for StoreError {}
impl From<rusqlite::Error> for StoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}
impl From<std::io::Error> for StoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Copy, Debug)]
pub struct StorePolicy {
    pub retention_days: u32,
    pub max_rows: u32,
    pub max_file_bytes: u64,
}

impl Default for StorePolicy {
    fn default() -> Self {
        Self {
            retention_days: 3,
            max_rows: 50_000,
            max_file_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SearchFilter {
    pub route: Option<RouteGroup>,
    pub status: Option<StatusGroup>,
    pub trace_id: Option<u128>,
    pub from_unix: Option<i64>,
    pub min_duration_us: Option<u64>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Clone, Debug)]
pub struct StoredTrace {
    pub observed_at_unix: i64,
    pub trace: RequestTrace,
}

#[derive(Debug)]
pub struct SearchPage {
    /// Exact count of matching roots in the stored, retained archive.
    pub matches: u64,
    /// Exact count of all roots in the retained archive, before filters.
    pub archived: u64,
    /// At most 100 results; an empty page does not mean zero matches.
    pub items: Vec<StoredTrace>,
}

#[derive(Clone, Copy, Debug)]
pub struct Maintenance {
    pub expired: usize,
    pub evicted: usize,
}

pub struct TraceStore {
    conn: Connection,
    policy: StorePolicy,
}

const CREATE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS traces (
    trace_id TEXT PRIMARY KEY,
    observed_at_unix INTEGER NOT NULL,
    route_group INTEGER NOT NULL CHECK(route_group BETWEEN 0 AND 4),
    status_group INTEGER NOT NULL CHECK(status_group BETWEEN 0 AND 4),
    duration_us INTEGER NOT NULL CHECK(duration_us >= 0),
    sqlx_known INTEGER NOT NULL CHECK(sqlx_known IN (0,1)),
    queries INTEGER NOT NULL CHECK(queries >= 0),
    query_elapsed_us INTEGER NOT NULL CHECK(query_elapsed_us >= 0),
    uncaptured INTEGER NOT NULL CHECK(uncaptured >= 0)
);
CREATE INDEX IF NOT EXISTS idx_traces_observed
ON traces(observed_at_unix DESC, trace_id DESC);
CREATE INDEX IF NOT EXISTS idx_traces_route_status
ON traces(route_group, status_group, observed_at_unix DESC);
CREATE TABLE IF NOT EXISTS query_spans (
    trace_id TEXT NOT NULL REFERENCES traces(trace_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 7),
    completed_at_us INTEGER NOT NULL CHECK(completed_at_us >= 0),
    elapsed_us INTEGER NOT NULL CHECK(elapsed_us >= 0),
    PRIMARY KEY(trace_id, ordinal)
);
"#;

impl TraceStore {
    pub fn open(path: &Path, policy: StorePolicy) -> StoreResult<Self> {
        if policy.retention_days == 0
            || policy.retention_days > 30
            || policy.max_rows == 0
            || policy.max_rows > 1_000_000
            || policy.max_file_bytes < 1_048_576
            || policy.max_file_bytes > 2 * 1024 * 1024 * 1024
        {
            return Err(StoreError::Invalid);
        }
        if let Ok(meta) = fs::symlink_metadata(path) {
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err(StoreError::Invalid);
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             PRAGMA secure_delete=ON;
             PRAGMA journal_mode=DELETE;
             PRAGMA busy_timeout=2000;",
        )?;
        conn.execute_batch(CREATE_SCHEMA)?;
        let page_size: u64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        let pages = policy.max_file_bytes / page_size;
        let current_pages: u64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
        if pages < current_pages {
            return Err(StoreError::Invalid);
        }
        conn.pragma_update(None, "max_page_count", pages)?;
        Ok(Self { conn, policy })
    }

    pub fn append(&mut self, trace: &RequestTrace, now_unix: i64) -> StoreResult<bool> {
        wire::encode(trace).map_err(|_| StoreError::Invalid)?;
        if now_unix < 0 {
            return Err(StoreError::Invalid);
        }
        let cutoff = now_unix.saturating_sub(i64::from(self.policy.retention_days) * 86_400);
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM traces WHERE observed_at_unix < ?1",
            params![cutoff],
        )?;
        let id = format!("{:032x}", trace.trace_id);
        let (sqlx_known, queries, query_us, uncaptured) = match &trace.sqlx {
            Some(sql) => (
                1_i64,
                sql.observed_queries as i64,
                sql.observed_elapsed_us as i64,
                sql.uncaptured_spans as i64,
            ),
            None => (0, 0, 0, 0),
        };
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO traces
             (trace_id,observed_at_unix,route_group,status_group,duration_us,
              sqlx_known,queries,query_elapsed_us,uncaptured)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                id,
                now_unix,
                trace.route as i64,
                trace.status as i64,
                trace.duration_us as i64,
                sqlx_known,
                queries,
                query_us,
                uncaptured
            ],
        )?;
        if inserted > 0 {
            if let Some(sql) = &trace.sqlx {
                for (ordinal, span) in sql.spans.iter().enumerate() {
                    tx.execute(
                        "INSERT INTO query_spans
                         (trace_id,ordinal,completed_at_us,elapsed_us)
                         VALUES (?1,?2,?3,?4)",
                        params![
                            id,
                            ordinal as i64,
                            span.completed_at_us as i64,
                            span.elapsed_us as i64
                        ],
                    )?;
                }
            }
        }
        tx.execute(
            "DELETE FROM traces WHERE trace_id IN
             (SELECT trace_id FROM traces ORDER BY observed_at_unix DESC,
              trace_id DESC LIMIT -1 OFFSET ?1)",
            params![i64::from(self.policy.max_rows)],
        )?;
        tx.commit()?;
        Ok(inserted > 0)
    }

    pub fn maintain(&mut self, now_unix: i64) -> StoreResult<Maintenance> {
        if now_unix < 0 {
            return Err(StoreError::Invalid);
        }
        let cutoff = now_unix.saturating_sub(i64::from(self.policy.retention_days) * 86_400);
        let expired = self.conn.execute(
            "DELETE FROM traces WHERE observed_at_unix < ?1",
            params![cutoff],
        )?;
        let evicted = self.conn.execute(
            "DELETE FROM traces WHERE trace_id IN
             (SELECT trace_id FROM traces ORDER BY observed_at_unix DESC,
              trace_id DESC LIMIT -1 OFFSET ?1)",
            params![i64::from(self.policy.max_rows)],
        )?;
        Ok(Maintenance { expired, evicted })
    }

    pub fn search(&self, filter: SearchFilter) -> StoreResult<SearchPage> {
        if filter
            .min_duration_us
            .is_some_and(|us| us > i64::MAX as u64)
        {
            return Err(StoreError::Invalid);
        }
        let route = filter.route.map(|r| r as i64);
        let status = filter.status.map(|s| s as i64);
        let id = filter.trace_id.map(|v| format!("{v:032x}"));
        let min_duration = filter.min_duration_us.map(|v| v as i64);
        let where_sql = "WHERE (?1 IS NULL OR route_group=?1)
            AND (?2 IS NULL OR status_group=?2)
            AND (?3 IS NULL OR trace_id=?3)
            AND (?4 IS NULL OR observed_at_unix>=?4)
            AND (?5 IS NULL OR duration_us>=?5)";
        let matches: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM traces {where_sql}"),
            params![route, status, id, filter.from_unix, min_duration],
            |row| row.get(0),
        )?;
        let archived: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM traces", [], |row| row.get(0))?;
        let limit = i64::from(filter.limit.clamp(1, 100));
        let offset = i64::from(filter.offset.min(1_000_000));
        let mut stmt = self.conn.prepare(&format!(
            "SELECT trace_id,observed_at_unix,route_group,status_group,
             duration_us,sqlx_known,queries,query_elapsed_us,uncaptured
             FROM traces {where_sql}
             ORDER BY observed_at_unix DESC,trace_id DESC LIMIT ?6 OFFSET ?7"
        ))?;
        let rows = stmt.query_map(
            params![
                route,
                status,
                id,
                filter.from_unix,
                min_duration,
                limit,
                offset
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            },
        )?;
        let base: Vec<_> = rows.collect::<Result<_, _>>()?;
        drop(stmt);
        let mut items = Vec::with_capacity(base.len());
        for (id, at, route, status, duration, known, count, elapsed, missing) in base {
            let route = match route {
                0 => RouteGroup::Home,
                1 => RouteGroup::Catalog,
                2 => RouteGroup::Books,
                3 => RouteGroup::Reader,
                4 => RouteGroup::Other,
                _ => return Err(StoreError::Invalid),
            };
            let status = match status {
                0 => StatusGroup::Success,
                1 => StatusGroup::Redirect,
                2 => StatusGroup::ClientError,
                3 => StatusGroup::ServerError,
                4 => StatusGroup::Other,
                _ => return Err(StoreError::Invalid),
            };
            let sqlx = if known == 0 {
                None
            } else {
                let mut spans = self.conn.prepare(
                    "SELECT completed_at_us,elapsed_us FROM query_spans
                     WHERE trace_id=?1 ORDER BY ordinal",
                )?;
                let entries = spans.query_map(params![id], |r| {
                    Ok(SqlSpan {
                        completed_at_us: r.get::<_, i64>(0)? as u64,
                        elapsed_us: r.get::<_, i64>(1)? as u64,
                    })
                })?;
                Some(SqlEvidence {
                    observed_queries: count as u64,
                    observed_elapsed_us: elapsed as u64,
                    spans: entries.collect::<Result<_, _>>()?,
                    uncaptured_spans: missing as u64,
                })
            };
            let trace_id = u128::from_str_radix(&id, 16).map_err(|_| StoreError::Invalid)?;
            items.push(StoredTrace {
                observed_at_unix: at,
                trace: RequestTrace {
                    trace_id,
                    route,
                    status,
                    duration_us: duration as u64,
                    sqlx,
                },
            });
        }
        Ok(SearchPage {
            matches: matches as u64,
            archived: archived as u64,
            items,
        })
    }
}
