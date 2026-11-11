//! Child attempts have their own bounded retention; a sampled-out, delayed or
//! expired HTTP root must not make genuine background evidence disappear.
use super::{StoreError, StoreResult, TraceStore};
use crate::services::trace::background::{
    lookup::{BackgroundPage, LIMIT},
    BackgroundTrace,
};
use rusqlite::{params, OptionalExtension};

pub(super) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS background_traces (
    trace_id TEXT PRIMARY KEY CHECK(length(trace_id)=32),
    parent_trace_id TEXT NOT NULL CHECK(length(parent_trace_id)=32),
    observed_at_unix INTEGER NOT NULL CHECK(observed_at_unix>=0),
    frame BLOB NOT NULL CHECK(length(frame)=80)
);
CREATE INDEX IF NOT EXISTS idx_background_parent
ON background_traces(parent_trace_id,observed_at_unix DESC,trace_id DESC);
CREATE INDEX IF NOT EXISTS idx_background_observed
ON background_traces(observed_at_unix DESC,trace_id DESC);
"#;

impl TraceStore {
    pub fn append_background(&mut self, trace: &BackgroundTrace, now: i64) -> StoreResult<bool> {
        let frame = trace.encode().ok_or(StoreError::Invalid)?;
        if now < 0 {
            return Err(StoreError::Invalid);
        }
        let cutoff = now.saturating_sub(i64::from(self.policy.retention_days) * 86_400);
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM background_traces WHERE observed_at_unix < ?1",
            params![cutoff],
        )?;
        let id = format!("{:032x}", trace.trace_id);
        let old: Option<Vec<u8>> = tx
            .query_row(
                "SELECT frame FROM background_traces WHERE trace_id=?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        if old.as_ref().is_some_and(|old| old.as_slice() != frame) {
            return Err(StoreError::Invalid);
        }
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO background_traces
             (trace_id,parent_trace_id,observed_at_unix,frame) VALUES (?1,?2,?3,?4)",
            params![
                id,
                format!("{:032x}", trace.parent_trace_id),
                now,
                frame.as_slice()
            ],
        )?;
        tx.execute(
            "DELETE FROM background_traces WHERE trace_id IN
             (SELECT trace_id FROM background_traces
              ORDER BY observed_at_unix DESC,trace_id DESC LIMIT -1 OFFSET ?1)",
            params![i64::from(self.policy.max_rows)],
        )?;
        tx.commit()?;
        Ok(inserted == 1)
    }

    pub(super) fn maintain_background(&mut self, now: i64) -> StoreResult<(usize, usize)> {
        let cutoff = now.saturating_sub(i64::from(self.policy.retention_days) * 86_400);
        let expired = self.conn.execute(
            "DELETE FROM background_traces WHERE observed_at_unix < ?1",
            params![cutoff],
        )?;
        let evicted = self.conn.execute(
            "DELETE FROM background_traces WHERE trace_id IN
             (SELECT trace_id FROM background_traces
              ORDER BY observed_at_unix DESC,trace_id DESC LIMIT -1 OFFSET ?1)",
            params![i64::from(self.policy.max_rows)],
        )?;
        Ok((expired, evicted))
    }

    pub fn background_for_parent(&self, parent: u128) -> StoreResult<BackgroundPage> {
        let id = format!("{parent:032x}");
        let matches: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM background_traces WHERE parent_trace_id=?1",
            params![id],
            |row| row.get(0),
        )?;
        let mut statement = self.conn.prepare(
            "SELECT observed_at_unix,frame FROM background_traces
             WHERE parent_trace_id=?1 ORDER BY observed_at_unix DESC,trace_id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![id, LIMIT as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (at, bytes) = row?;
            let trace = BackgroundTrace::decode(&bytes).ok_or(StoreError::Invalid)?;
            if at < 0 || trace.parent_trace_id != parent {
                return Err(StoreError::Invalid);
            }
            items.push((at, trace));
        }
        Ok(BackgroundPage {
            matches: matches as u64,
            items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{RouteGroup, StatusGroup},
        repositories::sqlite::{SearchFilter, StorePolicy},
        services::trace::{background::Outcome, RequestTrace},
    };
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "tor-background-{}-{}.sqlite",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
            )))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn child(id: u128) -> BackgroundTrace {
        BackgroundTrace {
            parent_trace_id: 16,
            trace_id: id,
            attempt: 1,
            outcome: Outcome::Completed,
            phase: 4,
            duration_us: 1_000,
            phase_us: [100, 200, 300, 50],
        }
    }

    #[test]
    fn delayed_root_and_reopen_preserve_separate_child_evidence() {
        let file = Temp::new();
        let policy = StorePolicy::default();
        let mut store = TraceStore::open(&file.0, policy).unwrap();
        assert!(store.append_background(&child(32), 100).unwrap());
        assert_eq!(store.search(SearchFilter::default()).unwrap().archived, 0);
        drop(store);
        let mut store = TraceStore::open(&file.0, policy).unwrap();
        assert_eq!(store.background_for_parent(16).unwrap().items[0].1, child(32));
        let root = RequestTrace {
            trace_id: 16,
            route: RouteGroup::Other,
            status: StatusGroup::Success,
            duration_us: 10,
            sqlx: None,
        };
        store.append(&root, 101).unwrap();
        assert_eq!(store.search(SearchFilter::default()).unwrap().archived, 1);
        assert_eq!(store.background_for_parent(16).unwrap().matches, 1);
        assert_eq!(store.background_for_parent(17).unwrap().matches, 0);
    }

    #[test]
    fn duplicate_is_idempotent_but_rewritten_receipt_is_rejected() {
        let file = Temp::new();
        let mut store = TraceStore::open(&file.0, StorePolicy::default()).unwrap();
        assert!(store.append_background(&child(32), 100).unwrap());
        assert!(!store.append_background(&child(32), 101).unwrap());
        let mut changed = child(32);
        changed.outcome = Outcome::Failed;
        assert!(store.append_background(&changed, 102).is_err());
        assert_eq!(store.background_for_parent(16).unwrap().items[0].1, child(32));
    }

    #[test]
    fn child_capacity_and_retention_do_not_depend_on_an_http_root() {
        let file = Temp::new();
        let policy = StorePolicy {
            max_rows: 2,
            retention_days: 1,
            ..StorePolicy::default()
        };
        let mut store = TraceStore::open(&file.0, policy).unwrap();
        for id in 32..35 {
            store.append_background(&child(id), 100 + id as i64).unwrap();
        }
        let page = store.background_for_parent(16).unwrap();
        assert_eq!(page.matches, 2);
        assert_eq!(page.items[0].1.trace_id, 34);
        store.maintain(100_000).unwrap();
        assert_eq!(store.background_for_parent(16).unwrap().matches, 0);
    }

    #[test]
    fn exact_count_is_not_lost_when_visible_children_are_bounded() {
        let file = Temp::new();
        let mut store = TraceStore::open(&file.0, StorePolicy::default()).unwrap();
        for id in 100..140 {
            store.append_background(&child(id), 100).unwrap();
        }
        let page = store.background_for_parent(16).unwrap();
        assert_eq!(page.matches, 40);
        assert_eq!(page.items.len(), LIMIT);
        let mut invalid = child(200);
        invalid.attempt = 0;
        assert!(store.append_background(&invalid, 100).is_err());
        assert_eq!(store.background_for_parent(16).unwrap().matches, 40);
    }
}
