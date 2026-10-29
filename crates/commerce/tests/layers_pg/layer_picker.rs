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
    pool.close().await;
}
