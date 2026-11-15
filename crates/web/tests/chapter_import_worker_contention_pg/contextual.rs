//! Current-model fixture for the unchanged shared-pool contention scenario.
use super::{isolated_psql, PgRecommendations};
use tor_recommendations::services::contextual::{Action, Control, MODEL};

pub(super) struct Fixture {
    generation: String,
    seed: i64,
    candidate: i64,
}

pub(super) async fn schedule(url: &str, nonce: u128, store: &PgRecommendations) -> Fixture {
    let before = store
        .contextual_status()
        .await
        .expect("current contextual policy");
    assert!(before.generations.iter().all(|g| g.status != "building"));
    let ids = isolated_psql(
        url,
        &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres)
         SELECT 'ci:mixed-contextual:{nonce}:'||n,'mix-{nonce}','Mixed contextual '||n,
           'public','basic',ARRAY['mixed-pool-metadata'] FROM generate_series(1,2) n RETURNING id"
    ),
    );
    let books: Vec<i64> = ids.lines().map(|id| id.parse().unwrap()).collect();
    assert_eq!(books.len(), 2);
    let generation = store
        .control_contextual(&Control {
            action: Action::Refresh,
            expected_revision: before.revision,
            generation: None,
        })
        .await
        .expect("schedule real contextual generation")
        .generation
        .unwrap();
    // Start at our own seed, not at arbitrary leftovers from another CI suite.
    // The runtime builder still computes the real bounded metadata page.
    isolated_psql(
        url,
        &format!(
            "UPDATE rec_generations SET cursor_book_id={} WHERE id='{generation}';
         INSERT INTO book_recommendation_relations(book_id,related_book_id) VALUES({},{})",
            books[0] - 1,
            books[0],
            books[1]
        ),
    );
    assert_eq!(
        isolated_psql(
            url,
            &format!("SELECT model_version FROM rec_generations WHERE id='{generation}'")
        ),
        MODEL
    );
    Fixture {
        generation,
        seed: books[0],
        candidate: books[1],
    }
}

pub(super) async fn verify_and_cleanup(url: &str, f: &Fixture, store: &PgRecommendations) {
    assert_eq!(
        isolated_psql(
            url,
            &format!(
                "SELECT cursor_book_id FROM rec_generations WHERE id='{}'",
                f.generation
            )
        ),
        f.seed.to_string(),
        "exactly one real seed must advance under contention"
    );
    assert_eq!(
        isolated_psql(
            url,
            &format!(
                "SELECT count(*) FROM rec_candidate_edges WHERE generation_id='{}'
          AND seed_id={} AND book_id={} AND
          ((source='content' AND reason='similar_by_content') OR
           (source='related_books' AND reason='manually_related'))",
                f.generation, f.seed, f.candidate
            )
        ),
        "2",
        "both real metadata and manual channels must execute"
    );
    let current = store.contextual_status().await.unwrap();
    store
        .control_contextual(&Control {
            action: Action::Cancel,
            expected_revision: current.revision,
            generation: Some(f.generation.clone()),
        })
        .await
        .expect("cancel only the owned partial contextual build");
    isolated_psql(
        url,
        &format!(
            "DELETE FROM rec_generations WHERE id='{}';
         DELETE FROM book_recommendation_relations WHERE book_id={};
         DELETE FROM books WHERE id IN ({},{})",
            f.generation, f.seed, f.seed, f.candidate
        ),
    );
    assert_eq!(
        isolated_psql(
            url,
            &format!(
                "SELECT count(*) FROM rec_generations WHERE id='{}'",
                f.generation
            )
        ),
        "0",
        "fixture must not leave a partially built generation"
    );
}
