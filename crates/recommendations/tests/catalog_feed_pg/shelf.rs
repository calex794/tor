//! Real PostgreSQL reader-end shelf: team links are relations, not ACL.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL with S028 teams"]
async fn reader_end_shelf_uses_live_author_team_other_authors_and_viewer_hides() {
    let f = fixture("reader-end-shelf").await;
    let (owner, teammate) = (f.actors[0], f.actors[1]);
    let (source, author_book, team_book, site_book) =
        (f.books[0], f.books[1], f.books[2], f.books[3]);

    sqlx::query(
        "UPDATE books SET owner_user_id=CASE
           WHEN id=ANY($1::bigint[]) THEN $2 ELSE $3 END,
           genres=ARRAY['fantasy'],tags=ARRAY['romance']
         WHERE id=ANY($4::bigint[])",
    )
    .bind([source, author_book])
    .bind(owner)
    .bind(teammate)
    .bind(&f.books)
    .execute(&f.pool)
    .await
    .unwrap();

    let team: i64 = sqlx::query_scalar(
        "INSERT INTO translation_teams(owner_user_id,name)
         VALUES($1,$2) RETURNING id",
    )
    .bind(owner)
    .bind(format!("Shelf integration {}", source))
    .fetch_one(&f.pool)
    .await
    .unwrap();

    let digest =
        tor_recommendations::domain::feedback::token_digest(&new_ticket().unwrap()).unwrap();
    sqlx::query(
        "INSERT INTO translation_team_invites
         (token_digest,team_id,invitee_user_id,inviter_user_id,status,decided_at)
         VALUES($1,$2,$3,$4,'accepted',now())",
    )
    .bind(digest.as_slice())
    .bind(team)
    .bind(teammate)
    .bind(owner)
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO translation_team_members(team_id,user_id,invite_digest)
         VALUES($1,$2,$3)",
    )
    .bind(team)
    .bind(teammate)
    .bind(digest.as_slice())
    .execute(&f.pool)
    .await
    .unwrap();

    for (book, added_by) in [(source, owner), (team_book, teammate)] {
        sqlx::query(
            "INSERT INTO translation_team_books(team_id,book_id,added_by_user_id)
             VALUES($1,$2,$3)",
        )
        .bind(team)
        .bind(book)
        .bind(added_by)
        .execute(&f.pool)
        .await
        .unwrap();
    }

    let result = f.store.reader_end_shelf(None, source, 200).await.unwrap();
    let source_for = |book: i64| {
        result
            .items
            .iter()
            .find(|item| item.book_id == book.to_string())
            .map(|item| item.source.as_str())
    };
    assert_eq!(source_for(author_book), Some("author"));
    assert_eq!(source_for(team_book), Some("teammate"));
    assert_eq!(source_for(site_book), Some("site"));
    assert!(!ids(&result).contains(&source));
    // An active editorial book is excluded in SQL before the organic
    // request fills the two reserved positions, without duplicating it.
    let mixed = f
        .store
        .reader_end_shelf_excluding(None, source, 204, &[author_book], 2)
        .await
        .unwrap();
    assert_eq!(mixed.items.len(), 2);
    assert!(!ids(&mixed).contains(&source));
    assert!(!ids(&mixed).contains(&author_book));
    assert_eq!(
        ids(&mixed).len(),
        ids(&mixed)
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
    // An other-author candidate is not guaranteed in any particular two-card
    // allocation: source quotas are averaged across rotations, not per shelf.
    assert_eq!(
        ids(&result).len(),
        ids(&result)
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );

    f.store
        .set_feedback(
            owner,
            site_book,
            Choice::NotInterested,
            0,
            &new_ticket().unwrap(),
        )
        .await
        .unwrap();
    assert!(!ids(&f
        .store
        .reader_end_shelf(Some(owner), source, 201)
        .await
        .unwrap())
    .contains(&site_book));
    assert!(ids(&f.store.reader_end_shelf(None, source, 201).await.unwrap()).contains(&site_book));

    // Historical nullable-owner fixtures are not eligible "other authors".
    // Do this after checking viewer-local hides against a valid owned Book.
    sqlx::query("UPDATE books SET owner_user_id=NULL WHERE id=$1")
        .bind(site_book)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(!ids(&f.store.reader_end_shelf(None, source, 203).await.unwrap()).contains(&site_book));

    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(team_book)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(!ids(&f.store.reader_end_shelf(None, source, 202).await.unwrap()).contains(&team_book));

    sqlx::query("DELETE FROM translation_team_books WHERE team_id=$1")
        .bind(team)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM translation_team_members WHERE team_id=$1")
        .bind(team)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM translation_team_invites WHERE team_id=$1")
        .bind(team)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM translation_teams WHERE id=$1")
        .bind(team)
        .execute(&f.pool)
        .await
        .unwrap();
    // An unowned source cannot establish owner/team/other-author categories.
    sqlx::query("UPDATE books SET owner_user_id=NULL WHERE id=$1")
        .bind(source)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f
        .store
        .reader_end_shelf(None, source, 205)
        .await
        .unwrap()
        .items
        .is_empty());

    sqlx::query("UPDATE books SET owner_user_id=NULL WHERE id=ANY($1::bigint[])")
        .bind(&f.books)
        .execute(&f.pool)
        .await
        .unwrap();
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL with 1000 discoverable books"]
async fn thousand_equal_relevance_books_are_not_limited_to_newest_hundred() {
    let mut f = fixture("shelf-depth").await;
    let source = f.books[0];
    // Every ordinary Book in the candidate pool has a real owner.
    // The 1,000 candidates all belong to another author, not the seed owner.
    sqlx::query(
        "UPDATE books SET owner_user_id=$1,genres=ARRAY['fantasy'],tags=ARRAY['system']
         WHERE id=$2",
    )
    .bind(f.actors[0])
    .bind(source)
    .execute(&f.pool)
    .await
    .unwrap();
    // The extra titles have the same language and relevance as the seed.
    // Insert in a single transaction; no real or shared database is touched.
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE books IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .unwrap();
    let generated: Vec<i64> = sqlx::query_scalar(
        "INSERT INTO books
            (id,source_identity,target_language,title,visibility,access_level,
             owner_user_id,genres,tags)
         SELECT (SELECT coalesce(max(id),0) FROM books)+n,
                'ci:shelf-depth:' || $1::text || ':' || n::text,
                'ru','Shelf depth candidate ' || n::text,'public','basic',
                $2,ARRAY['fantasy'],ARRAY['system']
         FROM generate_series(1,1000) AS gs(n) RETURNING id",
    )
    .bind(source)
    .bind(f.actors[1])
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(generated.len(), 1000);
    f.books.extend(generated);

    let mut observed = std::collections::BTreeSet::new();
    for _ in 0..50 {
        let feed = f.store.reader_end_shelf(None, source, 199).await.unwrap();
        let chosen = ids(&feed);
        assert_eq!(chosen.len(), 5);
        assert!(!chosen.contains(&source));
        assert_eq!(
            chosen
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            5
        );
        observed.extend(chosen);
    }
    // A SQL candidate window permanently capped at the newest 100 can
    // never pass this assertion, regardless of Rust-side tie shuffling.
    assert!(
        observed.len() > 100,
        "only {} distinct books were ever reachable across 50 requests",
        observed.len()
    );
    f.cleanup().await;
}
