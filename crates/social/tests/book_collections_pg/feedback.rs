//! Disposable-PG feedback acceptance: no real user data or grants.
use super::*;
use tor_social::repositories::collection_feedback::PgCollectionFeedback;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn e_07_public_feedback_is_scoped_idempotent_and_moderated() {
    let (pool, owner, reader, _, _, _) = fixture().await;
    let collections = PgBookCollections::new(pool.clone());
    let feedback = PgCollectionFeedback::new(pool.clone());
    let col = collections
        .create(owner, "Discussion", "private", &key(447))
        .await
        .unwrap();

    assert_eq!(
        feedback.save_rating(reader, col.id, 5).await,
        Err(CollectionError::NotFound)
    );
    assert_eq!(
        feedback
            .add_comment(reader, col.id, "Hidden", &key(81))
            .await,
        Err(CollectionError::NotFound)
    );
    assert!(feedback.read_public(col.id, None).await.is_err());
    collections
        .update_metadata_if_current(owner, col.id, 1, "Discussion", "public")
        .await
        .unwrap();
    assert_eq!(
        feedback.save_rating(reader, col.id, 0).await,
        Err(CollectionError::Invalid)
    );
    feedback.save_rating(reader, col.id, 5).await.unwrap();
    feedback.save_rating(owner, col.id, 1).await.unwrap();
    feedback.save_rating(owner, col.id, 3).await.unwrap();
    let malicious = "<script>alert(1)</script>";
    let id = feedback
        .add_comment(reader, col.id, malicious, &key(82))
        .await
        .unwrap();
    assert_eq!(
        feedback
            .add_comment(reader, col.id, malicious, &key(82))
            .await
            .unwrap(),
        id
    );
    assert_eq!(
        feedback
            .add_comment(reader, col.id, "Different", &key(82))
            .await,
        Err(CollectionError::Conflict)
    );
    let public = feedback.read_public(col.id, None).await.unwrap();
    assert_eq!(public.rating_count, 2);
    assert_eq!(public.average_rating, Some(4.0));
    assert_eq!(public.viewer_rating, None);
    assert_eq!(public.comments.len(), 1);
    assert_eq!(public.comments[0].body, malicious);
    assert_eq!(
        feedback
            .read_public(col.id, Some(reader))
            .await
            .unwrap()
            .viewer_rating,
        Some(5)
    );
    assert_eq!(
        feedback.delete_comment(900000000, col.id, id).await,
        Err(CollectionError::NotFound)
    );
    feedback.delete_comment(owner, col.id, id).await.unwrap();
    assert!(feedback
        .read_public(col.id, None)
        .await
        .unwrap()
        .comments
        .is_empty());
    assert_eq!(
        feedback.delete_comment(owner, col.id, id).await,
        Err(CollectionError::NotFound)
    );
    let second = feedback
        .add_comment(reader, col.id, "Author delete", &key(83))
        .await
        .unwrap();
    feedback
        .delete_comment(reader, col.id, second)
        .await
        .unwrap();

    collections
        .update_metadata_if_current(owner, col.id, 2, "Private", "private")
        .await
        .unwrap();
    assert!(feedback.read_public(col.id, Some(reader)).await.is_err());
    assert_eq!(
        feedback.save_rating(reader, col.id, 4).await,
        Err(CollectionError::NotFound)
    );
    assert_eq!(
        feedback.add_comment(reader, col.id, "No", &key(84)).await,
        Err(CollectionError::NotFound)
    );
    let purchases: i64 =
        sqlx::query_scalar("SELECT count(*) FROM library_purchased_chapters WHERE user_id=$1")
            .bind(reader)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(purchases, 0);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn e_07_concurrent_identical_comment_is_inserted_once() {
    let (pool, owner, reader, _, _, _) = fixture().await;
    let collections = PgBookCollections::new(pool.clone());
    let feedback = PgCollectionFeedback::new(pool.clone());
    let col = collections
        .create(owner, "Race comments", "public", &key(901))
        .await
        .unwrap();
    let token = key(902);
    let (first, second) = tokio::join!(
        feedback.add_comment(reader, col.id, "Once", &token),
        feedback.add_comment(reader, col.id, "Once", &token)
    );
    assert_eq!(first.unwrap(), second.unwrap());
    assert_eq!(
        feedback
            .read_public(col.id, None)
            .await
            .unwrap()
            .comments
            .len(),
        1
    );
}
