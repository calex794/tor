//! PERF-02c checks for the read-only social controls snapshot.
//! Kept out of slices_pg.rs to respect the repository file-size gate.

async fn perf02c_controls_before_follow(repo: &PgCreatorFollows) {
    assert_eq!(
        repo.book_page_follow_state(0, 825101).await,
        Err(CreatorFollowError::Invalid),
    );
    let page = repo.book_page_follow_state(825002, 825101).await.unwrap();
    assert!(!page.following_book);
    assert!(!page.is_owner);
    assert_eq!(page.creator_user_id, Some(825001));
    assert!(!page.following_creator);
    let owner = repo.book_page_follow_state(825001, 825101).await.unwrap();
    assert!(owner.is_owner);
    assert_eq!(owner.creator_user_id, Some(825001));
    assert!(!owner.following_creator);
    let draft = repo.book_page_follow_state(825003, 825102).await.unwrap();
    assert!(draft.is_owner, "legacy owner projection includes drafts");
    assert_eq!(draft.creator_user_id, None, "hidden book cannot expose creator");
}

async fn perf02c_controls_after_creator_follow(repo: &PgCreatorFollows, pool: &PgPool) {
    let page = repo.book_page_follow_state(825002, 825101).await.unwrap();
    assert!(page.following_creator);
    assert!(!page.following_book, "creator following is not a book follow");
    sqlx::query("INSERT INTO reader_book_follows(user_id,book_id) VALUES(825002,825101)")
        .execute(pool)
        .await
        .unwrap();
    assert!(
        repo.book_page_follow_state(825002, 825101)
            .await
            .unwrap()
            .following_book
    );
    sqlx::query("DELETE FROM reader_book_follows WHERE user_id=825002 AND book_id=825101")
        .execute(pool)
        .await
        .unwrap();
}

async fn perf02c_controls_after_hidden(repo: &PgCreatorFollows) {
    let page = repo.book_page_follow_state(825002, 825101).await.unwrap();
    assert_eq!(page.creator_user_id, None);
    assert!(!page.is_owner);
}
