//! Test-only cleanup of abandoned due S058 fixtures on the disposable PG DB.
//! Call only while holding the shared session advisory lock (580058).
use sqlx::PgPool;

pub(crate) async fn disable_abandoned_due_slots(pool: &PgPool) {
    // Interrupted tests can leave an enabled, overdue schedule behind. The
    // production worker has a GLOBAL queue: a later test must not accidentally
    // claim that orphan and mistake it for its own newly created Book.
    // Never consume occurrences or touch other tests' or user schedules.
    let disabled: Vec<i64> = sqlx::query_scalar(
        r#"UPDATE book_delayed_publication_settings s
           SET enabled=FALSE
           FROM books b
           WHERE b.id=s.book_id AND s.enabled
             AND s.next_slot_at<=clock_timestamp()
             AND b.source_identity LIKE ANY (ARRAY[
                 'tor-ci:s058-release:%',
                 'tor-ci:s058-frontier:%',
                 'tor-ci:s058-occurrence:%',
                 'tor-ci:s058-queue-stress:%',
                 'ci:s058-paid:%',
                 'ci:s058-paid-block:%'
             ])
           RETURNING s.book_id"#,
    )
    .fetch_all(pool)
    .await
    .expect("clean disposable S058 test fixtures");
    if !disabled.is_empty() {
        eprintln!("S058 cleaned abandoned due test books: {disabled:?}");
    }
}
