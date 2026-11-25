//! ADAPT-02e: explain the exact production candidate INSERT on disposable data.
use super::*;
use sqlx::{Postgres, Transaction};

const SOURCES: &str = include_str!("../../../src/repositories/feedback/sources.rs");
const EXPLAIN: &str = "EXPLAIN (ANALYZE, BUFFERS, WAL, SETTINGS, FORMAT TEXT) ";

fn content_insert() -> String {
    // Fail loudly if the production statement changes shape, rather than silently
    // measuring a stale copy. Neither the runtime SQL nor its deadline is changed.
    let (_, function) = SOURCES
        .split_once("pub(super) async fn book_sources(")
        .unwrap();
    let (_, body) = function.split_once("\"WITH candidates AS (").unwrap();
    let (body, _) = body.split_once("\")").unwrap();
    format!("WITH candidates AS ({body}")
}

async fn explain(
    tx: &mut Transaction<'_, Postgres>,
    statement: &str,
    generation: Option<&str>,
    seed: i64,
    cohort: &str,
    operation: &str,
) {
    let sql = format!("{EXPLAIN}{statement}");
    let rows: Vec<String> = match generation {
        Some(id) => sqlx::query_scalar(&sql)
            .bind(id)
            .bind(seed)
            .fetch_all(&mut **tx)
            .await
            .unwrap(),
        None => sqlx::query_scalar(&sql)
            .bind(seed)
            .fetch_all(&mut **tx)
            .await
            .unwrap(),
    };
    eprintln!("ADAPT_PLAN cohort={cohort} operation={operation}");
    for row in rows {
        eprintln!("ADAPT_PLAN {row}");
    }
}

pub(super) async fn run(f: &Fixture, language: &str) {
    let insert = content_insert();
    let (cte, _) = insert
        .split_once(" INSERT INTO rec_candidate_edges")
        .unwrap();
    let select = format!(
        "{} SELECT id,access_level,score FROM ranked WHERE position<=20",
        cte.replace("$2", "$1")
    );
    let generation = command(f, Action::Refresh, None).await;
    let seed = f.books[0];
    let before: i64 =
        sqlx::query_scalar("SELECT cursor_book_id FROM rec_generations WHERE id=$1::uuid")
            .bind(&generation)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    for (cohort, sparse, expected) in [("dense", false, 100_004), ("sparse", true, 12)] {
        let mut tx = f.pool.begin().await.unwrap();
        // EXPLAIN ANALYZE executes the INSERT. Only this rolled-back diagnostic
        // transaction receives a longer deadline, so expensive plans are visible.
        sqlx::query("SET LOCAL statement_timeout='15s'")
            .execute(&mut *tx)
            .await
            .unwrap();
        if sparse {
            let marker = format!("adapt-sparse-{seed}");
            let changed = sqlx::query(
                "UPDATE books SET genres=ARRAY[$2::text],tags=ARRAY[$2::text]
                 WHERE id=$1 OR id IN (
                   SELECT id FROM books WHERE target_language=$3
                   AND source_identity LIKE 'ci:contextual-scale:%' ORDER BY id LIMIT 12
                 )",
            )
            .bind(seed)
            .bind(marker)
            .bind(language)
            .execute(&mut *tx)
            .await
            .unwrap();
            assert_eq!(changed.rows_affected(), 13);
        }
        let candidates: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rec_materializable_books s
             JOIN rec_materializable_books b ON b.target_language=s.target_language AND b.id<>s.id
             WHERE s.id=$1 AND (b.genres && s.genres OR b.tags && s.tags)",
        )
        .bind(seed)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(candidates, expected);
        eprintln!("ADAPT_PLAN cohort={cohort} candidates={candidates}");
        explain(&mut tx, &select, None, seed, cohort, "select").await;
        explain(&mut tx, &insert, Some(&generation), seed, cohort, "insert").await;
        let edges: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rec_candidate_edges
             WHERE generation_id=$1::uuid AND seed_id=$2 AND source='content'",
        )
        .bind(&generation)
        .bind(seed)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(edges, expected.min(20));
        tx.rollback().await.unwrap();
        let state: (i64, i64) = sqlx::query_as(
            "SELECT cursor_book_id,(SELECT count(*) FROM rec_candidate_edges
             WHERE generation_id=$1::uuid) FROM rec_generations WHERE id=$1::uuid",
        )
        .bind(&generation)
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(state, (before, 0));
    }
    command(f, Action::Cancel, Some(&generation)).await;
    stop(f, &[generation]).await;
}
