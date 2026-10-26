use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use tor_observability::{
    domain::{RouteGroup, StatusGroup},
    repositories::sqlite::{SearchFilter, StorePolicy, TraceStore},
    services::trace::{RequestTrace, SqlEvidence, SqlSpan},
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TemporaryStore {
    dir: PathBuf,
}

impl TemporaryStore {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "tor-observability-{}-{stamp}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        Self { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("traces.sqlite3")
    }
}

impl Drop for TemporaryStore {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn example(id: u128, status: StatusGroup) -> RequestTrace {
    RequestTrace {
        trace_id: id,
        route: RouteGroup::Reader,
        status,
        duration_us: 20_000,
        sqlx: Some(SqlEvidence {
            observed_queries: 3,
            observed_elapsed_us: 9_000,
            spans: vec![
                SqlSpan {
                    completed_at_us: 5_000,
                    elapsed_us: 4_000,
                },
                SqlSpan {
                    completed_at_us: 15_000,
                    elapsed_us: 5_000,
                },
            ],
            uncaptured_spans: 1,
        }),
    }
}

fn policy(max_rows: u32) -> StorePolicy {
    StorePolicy {
        max_rows,
        max_file_bytes: 2 * 1024 * 1024,
        ..StorePolicy::default()
    }
}

#[test]
fn archives_survive_reopen_and_searches_cover_more_than_last_hundred() {
    let dir = TemporaryStore::new();
    let mut db = TraceStore::open(&dir.path(), policy(200)).unwrap();
    for id in 1..=150_u128 {
        assert!(db.append(&example(id, StatusGroup::ServerError), 1_800_000_000 + id as i64).unwrap());
    }
    let page = db.search(SearchFilter {
        limit: 100,
        ..SearchFilter::default()
    }).unwrap();
    assert_eq!(page.archived, 150);
    assert_eq!(page.matches, 150);
    assert_eq!(page.items.len(), 100);
    // The oldest trace is not in the first 100, but still searchable.
    let oldest = db.search(SearchFilter {
        trace_id: Some(1),
        limit: 10,
        ..SearchFilter::default()
    }).unwrap();
    assert_eq!(oldest.matches, 1);
    assert_eq!(oldest.items.len(), 1);
    assert_eq!(oldest.items[0].trace, example(1, StatusGroup::ServerError));
    drop(db);

    let reopened = TraceStore::open(&dir.path(), policy(200)).unwrap();
    assert_eq!(reopened.search(SearchFilter::default()).unwrap().archived, 150);
}

#[test]
fn retention_and_row_cap_delete_associated_child_spans() {
    let dir = TemporaryStore::new();
    let mut db = TraceStore::open(&dir.path(), policy(7)).unwrap();
    for id in 1..=20_u128 {
        db.append(&example(id, StatusGroup::Success), 1_900_000_000 + id as i64)
            .unwrap();
    }
    let page = db.search(SearchFilter::default()).unwrap();
    assert_eq!(page.archived, 7);
    assert_eq!(page.items.len(), 1); // default limit is one
    assert_eq!(page.items[0].trace.trace_id, 20);
    assert_eq!(db.search(SearchFilter {
        trace_id: Some(1),
        ..SearchFilter::default()
    }).unwrap().matches, 0);

    let report = db.maintain(1_900_000_000 + 5 * 86_400).unwrap();
    assert_eq!(report.expired, 7);
    assert_eq!(db.search(SearchFilter::default()).unwrap().archived, 0);
    drop(db);
    let raw = rusqlite::Connection::open(dir.path()).unwrap();
    let count: i64 = raw
        .query_row("SELECT count(*) FROM query_spans", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn idempotent_trace_id_and_missing_observer_are_preserved() {
    let dir = TemporaryStore::new();
    let mut db = TraceStore::open(&dir.path(), policy(8)).unwrap();
    let mut unknown = example(16, StatusGroup::ServerError);
    unknown.sqlx = None;
    assert!(db.append(&unknown, 1_900_000_000).unwrap());
    assert!(!db.append(&example(16, StatusGroup::ClientError), 1_900_000_100).unwrap());
    let page = db.search(SearchFilter {
        trace_id: Some(16),
        ..SearchFilter::default()
    }).unwrap();
    assert_eq!(page.matches, 1);
    assert_eq!(page.items[0].trace.sqlx, None);
    assert_eq!(page.items[0].trace.status, StatusGroup::ServerError);
}

#[test]
fn filtered_search_reports_exact_archive_scope_and_never_stores_raw_strings() {
    let dir = TemporaryStore::new();
    let mut db = TraceStore::open(&dir.path(), policy(16)).unwrap();
    db.append(&example(16, StatusGroup::Success), 1_900_000_000).unwrap();
    db.append(&example(17, StatusGroup::ServerError), 1_900_000_001).unwrap();
    let page = db.search(SearchFilter {
        route: Some(RouteGroup::Reader),
        status: Some(StatusGroup::ServerError),
        min_duration_us: Some(10_000),
        from_unix: Some(1_900_000_000),
        ..SearchFilter::default()
    }).unwrap();
    assert_eq!(page.archived, 2);
    assert_eq!(page.matches, 1);
    assert_eq!(page.items[0].trace.trace_id, 17);
    drop(db);
    let bytes = fs::read(dir.path()).unwrap();
    for sensitive in [b"user_id".as_slice(), b"remote_ip", b"raw_url", b"SECRET"] {
        assert!(!bytes.windows(sensitive.len()).any(|window| window == sensitive));
    }
}

#[test]
fn invalid_limits_and_symlinks_do_not_open_unsafe_store() {
    let dir = TemporaryStore::new();
    assert!(TraceStore::open(&dir.path(), StorePolicy {
        max_rows: 0,
        ..policy(10)
    }).is_err());
    assert!(TraceStore::open(&dir.path(), StorePolicy {
        max_file_bytes: 100,
        ..policy(10)
    }).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let target = dir.dir.join("missing.sqlite3");
        let link = dir.dir.join("symlink.sqlite3");
        symlink(&target, &link).unwrap();
        assert!(TraceStore::open(&link, policy(10)).is_err());
    }
}
