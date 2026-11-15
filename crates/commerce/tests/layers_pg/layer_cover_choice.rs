//! Choice is cosmetic, owner-scoped, and reuses a B-attached verified Book cover.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_package_cover_choice_preserves_versions_and_rejects_unapproved() {
    let (pool, repo, owner, outsider, book) = setup("package-cover-choice").await;
    let layer = repo.create_layer(id(owner), "Cover choice").await.unwrap();
    let version = repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    let before: (i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT current_version_id,metadata_revision,cover_book_id
         FROM commerce_layers WHERE id=$1",
    )
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, (version, 1, None));
    assert_eq!(
        repo.choose_package_cover(id(owner), id(layer), id(book), 1)
            .await,
        Err(LayerError::Inaccessible),
        "a book without an approved attached cover is ineligible",
    );
    assert_eq!(
        repo.choose_package_cover(id(outsider), id(layer), id(book), 1)
            .await,
        Err(LayerError::Inaccessible),
    );

    let source: i64 = sqlx::query_scalar(
        "INSERT INTO cover_upload_objects(
            book_id,uploader_user_id,storage_key,content_sha256,
            content_type,size_bytes,status
         ) VALUES($1,$2,$3,decode(repeat('ab',32),'hex'),
                  'image/png',100,'verified') RETURNING id",
    )
    .bind(book)
    .bind(owner)
    .bind(format!("staging/covers/sources/package-choice/{book}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let job: i64 = sqlx::query_scalar(
        "INSERT INTO cover_variant_jobs(
            source_object_id,book_id,actor_user_id,recipe_version,
            expected_slot_revision,status
         ) VALUES($1,$2,$3,'package-cover-test-v1',0,'ready') RETURNING id",
    )
    .bind(source)
    .bind(book)
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    for variant in ["card", "main"] {
        sqlx::query(
            "INSERT INTO cover_variants(
                job_id,variant_name,storage_key,content_sha256,size_bytes
             ) VALUES($1,$2,$3,decode(repeat('ab',32),'hex'),100)",
        )
        .bind(job)
        .bind(variant)
        .bind(format!(
            "staging/covers/variants/package-choice/{book}/{variant}"
        ))
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO book_cover_slots(book_id,job_id) VALUES($1,$2)")
        .bind(book)
        .bind(job)
        .execute(&pool)
        .await
        .unwrap();

    repo.choose_package_cover(id(owner), id(layer), id(book), 1)
        .await
        .unwrap();
    let after: (i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT current_version_id,metadata_revision,cover_book_id
         FROM commerce_layers WHERE id=$1",
    )
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, (version, 2, Some(book)));
    assert_eq!(
        repo.choose_package_cover(id(owner), id(layer), id(book), 1)
            .await,
        Err(LayerError::StaleOffer),
    );
    let version_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commerce_layer_versions WHERE layer_id=$1")
            .bind(layer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        version_count, 2,
        "cover selection must not create a new composition version"
    );
    pool.close().await;
}
