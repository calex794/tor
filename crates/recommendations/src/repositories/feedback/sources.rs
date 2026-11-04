//! Background-only source materialization. No graph scan is run by an HTTP handler.
use super::*;

pub(super) async fn global_sources(
    tx: &mut Transaction<'_, Postgres>,
    generation: &str,
) -> Result<(), FeedbackError> {
    let signals:i64=sqlx::query_scalar("SELECT count(*) FROM rec_reader_signals s JOIN rec_generations g ON g.id=$1::uuid WHERE s.occurred_at<=g.source_window_end AND s.occurred_at>g.source_window_end-interval '30 days'")
        .bind(generation).fetch_one(&mut **tx).await?;
    if signals > 50_000 {
        return Err(FeedbackError::Limited);
    }
    // Only consenting, independently C-verified completed books enter the graph.
    // NEwBOO transition confidence: <=30min full; <=6h 30%; otherwise none.
    sqlx::query(
        "WITH ordered AS (
           SELECT s.book_id,s.occurred_at,
             lag(s.book_id) OVER(PARTITION BY s.subject_id ORDER BY s.occurred_at,s.source_event_id) AS previous_book,
             lag(s.occurred_at) OVER(PARTITION BY s.subject_id ORDER BY s.occurred_at,s.source_event_id) AS previous_time
           FROM rec_reader_signals s JOIN rec_generations g ON g.id=$1::uuid
           WHERE s.occurred_at<=g.source_window_end AND s.occurred_at>g.source_window_end-interval '30 days'
         ), counts AS (
           SELECT previous_book AS seed_id,book_id,
             sum(CASE WHEN occurred_at-previous_time<=interval '30 minutes' THEN 700 ELSE 210 END)::bigint AS weight
           FROM ordered WHERE previous_book IS NOT NULL AND previous_book<>book_id
             AND occurred_at>previous_time AND occurred_at-previous_time<=interval '6 hours'
             AND EXISTS(SELECT 1 FROM rec_public_books WHERE id=book_id)
             AND EXISTS(SELECT 1 FROM rec_public_books WHERE id=previous_book)
           GROUP BY previous_book,book_id
         ), ranked AS (
           SELECT *,row_number() OVER(PARTITION BY seed_id ORDER BY weight DESC,book_id::text) AS position,
             max(weight) OVER(PARTITION BY seed_id) AS maximum FROM counts
         ) INSERT INTO rec_candidate_edges(generation_id,seed_id,book_id,source,reason,score)
         SELECT $1::uuid,seed_id,book_id,'behavioral','readers_also_read',GREATEST(1,weight*1000/maximum)::smallint
         FROM ranked WHERE position<=20 ON CONFLICT DO NOTHING")
        .bind(generation).execute(&mut **tx).await?;
    sqlx::query(
        "WITH counts AS (
          SELECT s.book_id,count(*)::bigint AS weight FROM rec_reader_signals s
          JOIN rec_generations g ON g.id=$1::uuid JOIN rec_public_books b ON b.id=s.book_id
          WHERE s.occurred_at<=g.source_window_end AND s.occurred_at>g.source_window_end-interval '7 days'
          GROUP BY s.book_id
        ), ranked AS (SELECT *,max(weight) OVER() AS maximum FROM counts)
        INSERT INTO rec_candidate_edges(generation_id,seed_id,book_id,source,reason,score)
        SELECT $1::uuid,0,book_id,'popularity','popular',GREATEST(1,weight*1000/maximum)::smallint
        FROM ranked ORDER BY weight DESC,book_id::text LIMIT 20 ON CONFLICT DO NOTHING")
        .bind(generation).execute(&mut **tx).await?;
    // New releases are explicitly labelled, never fabricated popularity/read facts.
    sqlx::query(
        "INSERT INTO rec_candidate_edges(generation_id,seed_id,book_id,source,reason,score)
         SELECT $1::uuid,0,b.id,'popularity','new_release',500
         FROM rec_public_books b WHERE NOT EXISTS(SELECT 1 FROM rec_candidate_edges WHERE generation_id=$1::uuid AND seed_id=0)
         ORDER BY b.created_at DESC,b.id DESC LIMIT 20 ON CONFLICT DO NOTHING")
        .bind(generation).execute(&mut **tx).await?;
    Ok(())
}

pub(super) async fn book_sources(
    tx: &mut Transaction<'_, Postgres>,
    generation: &str,
    seed: i64,
) -> Result<(), FeedbackError> {
    let cte = super::discovery::GENERATION_BOOKS_CTE;
    // Exact NEwBOO parity weights: genre 180 (max 3), tag 120 (max 5).
    // Preserve the old 20 basic candidates while allowing up to 20 per
    // restricted tier. Anonymous reads still use the live basic-only gate.
    let content_sql = format!(
        "{cte}, candidates AS (
           SELECT b.id,b.access_level,
             LEAST(1000,
               LEAST(3,(SELECT count(*) FROM (SELECT unnest(b.genres) INTERSECT SELECT unnest(s.genres)) x))*180 +
               LEAST(5,(SELECT count(*) FROM (SELECT unnest(b.tags) INTERSECT SELECT unnest(s.tags)) x))*120 +10
             )::smallint AS score
           FROM rec_generation_books s
           JOIN rec_generation_books b
             ON b.target_language=s.target_language AND b.id<>s.id
           WHERE s.id=$2 AND (b.genres && s.genres OR b.tags && s.tags)
         ), ranked AS (
           SELECT id,score,row_number() OVER(
             PARTITION BY access_level ORDER BY score DESC,id::text
           ) AS position FROM candidates
         )
         INSERT INTO rec_candidate_edges(generation_id,seed_id,book_id,source,reason,score)
         SELECT $1::uuid,$2,id,'content','similar_by_content',score
         FROM ranked WHERE position<=20
         ORDER BY score DESC,id::text ON CONFLICT DO NOTHING"
    );
    sqlx::query(&content_sql)
        .bind(generation)
        .bind(seed)
        .execute(&mut **tx)
        .await?;

    // Manual Book relations are author/editor-managed metadata. A related
    // adult/pro_adult target remains invisible to guests until live A rights
    // are verified by the final REC metadata query and ticket activation.
    let related_sql = format!(
        "{cte} INSERT INTO rec_candidate_edges(
             generation_id,seed_id,book_id,source,reason,score
         )
         SELECT $1::uuid,$2,r.related_book_id,'related_books','manually_related',1000
         FROM book_recommendation_relations r
         JOIN rec_generation_books b ON b.id=r.related_book_id
         WHERE r.book_id=$2
           AND EXISTS(SELECT 1 FROM rec_generation_books WHERE id=$2)
         ORDER BY r.related_book_id::text LIMIT 10 ON CONFLICT DO NOTHING"
    );
    sqlx::query(&related_sql)
        .bind(generation)
        .bind(seed)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
