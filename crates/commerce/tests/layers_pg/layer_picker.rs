//! D-05: the browser's eligible list and locked add_book() MUST agree.
use super::*;

async fn own_book(pool: &PgPool, owner: i64, suffix: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru',$2,'public',$3) RETURNING id",
    )
    .bind(format!("ci:d05:picker:{suffix}:{owner}"))
    .bind(format!("D05 picker {suffix}"))
    .bind(owner)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_picker_lists_only_single_active_owner_books_and_rechecks_on_add() {
    let (pool, repo, owner, collaborator, solo_book) = setup("picker-only-owner").await;
    let layer = repo
        .create_layer(id(owner), "Picker only owner")
        .await
        .unwrap();
    let summaries = repo.owned_layers(id(owner), 100).await.unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].layer_id, layer);
    assert_eq!(summaries[0].title, "Picker only owner");
    assert!(repo
        .owned_layers(id(collaborator), 100)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        repo.owned_layers(id(owner), 0).await,
        Err(LayerError::Invalid)
    );
    let team_book = own_book(&pool, owner, "team").await;
    let held_book = own_book(&pool, owner, "held").await;
    let other_owner_book = own_book(&pool, collaborator, "other-owner").await;

    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(held_book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',TRUE)",
    )
    .bind(team_book)
    .bind(collaborator)
    .execute(&pool)
    .await
    .unwrap();

    let first = repo
        .addable_books(id(owner), id(layer), None, 50)
        .await
        .unwrap();
    assert_eq!(
        first.books.iter().map(|b| b.book_id).collect::<Vec<_>>(),
        vec![solo_book],
        "active collaborator, admin hold and foreign owner are not offered"
    );
    assert!(first.next_after.is_none());
    assert_eq!(
        repo.addable_books(id(collaborator), id(layer), None, 50)
            .await,
        Err(LayerError::Inaccessible),
        "a foreign actor must not enumerate the owner's private Books"
    );
    assert_eq!(
        repo.addable_books(id(owner), id(layer), None, 0).await,
        Err(LayerError::Invalid)
    );
    assert_eq!(
        repo.addable_books(id(owner), id(layer), None, 51).await,
        Err(LayerError::Invalid)
    );
    assert!(!first.books.iter().any(|b| b.book_id == other_owner_book));

    // An inactive translator does not count as a team seat.
    sqlx::query("UPDATE book_members SET is_active=FALSE WHERE book_id=$1 AND user_id=$2")
        .bind(team_book)
        .bind(collaborator)
        .execute(&pool)
        .await
        .unwrap();
    let all = repo
        .addable_books(id(owner), id(layer), None, 50)
        .await
        .unwrap();
    assert_eq!(
        all.books.iter().map(|b| b.book_id).collect::<Vec<_>>(),
        vec![solo_book, team_book],
    );

    // One-row keyset pages remain stable and never reveal another owner's Book.
    let page1 = repo
        .addable_books(id(owner), id(layer), None, 1)
        .await
        .unwrap();
    assert_eq!(page1.books[0].book_id, solo_book);
    let page2 = repo
        .addable_books(id(owner), id(layer), page1.next_after.map(id), 1)
        .await
        .unwrap();
    assert_eq!(page2.books[0].book_id, team_book);
    assert!(page2.next_after.is_none());

    // The selected option may become ineligible before the form POST.
    sqlx::query("UPDATE book_members SET is_active=TRUE WHERE book_id=$1 AND user_id=$2")
        .bind(team_book)
        .bind(collaborator)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.add_book(id(owner), id(layer), id(team_book)).await,
        Err(LayerError::Inaccessible),
        "stale UI cannot bypass active membership"
    );
    sqlx::query("UPDATE book_members SET is_active=FALSE WHERE book_id=$1 AND user_id=$2")
        .bind(team_book)
        .bind(collaborator)
        .execute(&pool)
        .await
        .unwrap();
    repo.add_book(id(owner), id(layer), id(team_book))
        .await
        .unwrap();
    assert!(
        repo.addable_books(id(owner), id(layer), None, 50)
            .await
            .unwrap()
            .books
            .iter()
            .all(|book| book.book_id != team_book),
        "already-included Books are never offered as available"
    );
    assert!(
        sqlx::query("UPDATE book_members SET is_active=TRUE WHERE book_id=$1 AND user_id=$2")
            .bind(team_book)
            .bind(collaborator)
            .execute(&pool)
            .await
            .is_err(),
        "SQL trigger keeps the unique-owner rule after inclusion"
    );
    let visible = repo
        .current_books(id(owner), id(layer), None, 50)
        .await
        .unwrap();
    assert_eq!(visible.books.len(), 1);
    assert_eq!(visible.books[0].book_id, team_book);
    assert_eq!(
        repo.current_books(id(collaborator), id(layer), None, 50)
            .await,
        Err(LayerError::Inaccessible)
    );
    assert_eq!(
        repo.current_books(id(owner), id(layer), None, 0).await,
        Err(LayerError::Invalid)
    );

    // A second write makes the reader's earlier remove form stale.
    let v2 = visible.current_version_id;
    repo.add_book(id(owner), id(layer), id(solo_book))
        .await
        .unwrap();
    let newer = repo
        .current_books(id(owner), id(layer), None, 1)
        .await
        .unwrap();
    assert_eq!(newer.books.len(), 1);
    assert_eq!(newer.books[0].book_id, solo_book);
    assert!(newer.next_after.is_some());
    let second_page = repo
        .current_books(id(owner), id(layer), newer.next_after.map(id), 1)
        .await
        .unwrap();
    assert_eq!(second_page.books[0].book_id, team_book);
    assert_eq!(
        repo.remove_book_if_current(id(owner), id(layer), id(team_book), id(v2))
            .await,
        Err(LayerError::StaleOffer),
        "a stale HTTP form must not remove a Book from a newer composition"
    );
    let unchanged: i64 =
        sqlx::query_scalar("SELECT current_version_id FROM commerce_layers WHERE id=$1")
            .bind(layer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unchanged, newer.current_version_id);
    assert_eq!(
        repo.remove_book_if_current(
            id(collaborator),
            id(layer),
            id(team_book),
            id(newer.current_version_id),
        )
        .await,
        Err(LayerError::Inaccessible)
    );
    repo.remove_book_if_current(
        id(owner),
        id(layer),
        id(team_book),
        id(newer.current_version_id),
    )
    .await
    .unwrap();
    let historical: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_layer_version_books
         WHERE layer_version_id=$1 AND book_id=$2",
    )
    .bind(newer.current_version_id)
    .bind(team_book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(historical, 1, "historical versions are immutable");
    let current = repo
        .current_books(id(owner), id(layer), None, 50)
        .await
        .unwrap();
    assert_eq!(
        current.books.iter().map(|b| b.book_id).collect::<Vec<_>>(),
        vec![solo_book],
    );
    // Once removed from the current composition, the Book may gain a team.
    sqlx::query("UPDATE book_members SET is_active=TRUE WHERE book_id=$1 AND user_id=$2")
        .bind(team_book)
        .bind(collaborator)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        repo.addable_books(id(owner), id(layer), None, 50)
            .await
            .unwrap()
            .books
            .iter()
            .all(|b| b.book_id != team_book),
        "reactivated collaborator keeps a removed Book out of the picker"
    );
    pool.close().await;
}


#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_membership_identity_cannot_move_out_of_live_layer() {
    let (pool, repo, owner, collaborator, book) = setup("member-identity-fence").await;
    let layer = repo.create_layer(id(owner), "Membership fence").await.unwrap();
    repo.add_book(id(owner), id(layer), id(book)).await.unwrap();
    let unrelated = own_book(&pool, collaborator, "member-identity-unrelated").await;
    // A direct DML update of the primary-key identity must not migrate
    // active membership out of Layer without taking the Book row lock.
    let moved = sqlx::query(
        "UPDATE book_members SET book_id=$2 WHERE book_id=$1 AND user_id=$3",
    )
    .bind(book)
    .bind(unrelated)
    .bind(owner)
    .execute(&pool)
    .await;
    assert!(moved.is_err(), "Layer member identity update must be rejected");
    let swapped = sqlx::query(
        "UPDATE book_members SET user_id=$2 WHERE book_id=$1 AND user_id=$3",
    )
    .bind(book)
    .bind(collaborator)
    .bind(owner)
    .execute(&pool)
    .await;
    assert!(swapped.is_err(), "Layer member actor update must be rejected");
    pool.close().await;
}
