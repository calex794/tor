//! S020: reciprocal related-book creation, duplicate-safe concurrent adds,
//! owner checks and private-target non-disclosure on real disposable PG.
use sqlx::PgPool;
use tor_books::repositories::{book_members::PgBookMembers, postgres::PgAuthoring, PgCatalog};
use tor_books::services::authoring::AuthorError;
use tor_contracts::access::BookRole;

#[tokio::test]
#[ignore = "requires local isolated /tor_ci PostgreSQL"]
async fn s020_related_books_are_bilateral_and_never_expose_private_targets() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL URL");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    let pool = PgPool::connect(&url).await.unwrap();
    let mark = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Related Author','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-related-owner-{mark}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let another: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Related Other','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-related-other-{mark}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();

    let translator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Related Translator','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-related-translator-{mark}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let moderator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Related Moderator','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-related-moderator-{mark}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();

    async fn make_book(
        pool: &PgPool,
        identity: String,
        title: String,
        owner: i64,
        visibility: &str,
        held: bool,
    ) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                               visibility,access_level,admin_hold)
             VALUES($1,'ru',$2,$3,$4,'basic',$5) RETURNING id",
        )
        .bind(identity)
        .bind(title)
        .bind(owner)
        .bind(visibility)
        .bind(held)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    let a_title = format!("S020 Связанное произведение {mark}");
    let b_title = format!("S020 Продолжение {mark}");
    let private_title = format!("S020 Скрытое продолжение {mark}");
    let held_title = format!("S020 Заблокированное {mark}");
    let a = make_book(
        &pool,
        format!("related:a:{mark}"),
        a_title.clone(),
        owner,
        "public",
        false,
    )
    .await;
    let b = make_book(
        &pool,
        format!("related:b:{mark}"),
        b_title.clone(),
        another,
        "public",
        false,
    )
    .await;
    let hidden = make_book(
        &pool,
        format!("related:private:{mark}"),
        private_title.clone(),
        another,
        "draft",
        false,
    )
    .await;
    let held = make_book(
        &pool,
        format!("related:held:{mark}"),
        held_title.clone(),
        another,
        "public",
        true,
    )
    .await;
    let repo = PgAuthoring::new(pool.clone());
    let members = PgBookMembers::new(pool.clone());
    members
        .add_collaborator(owner, a, translator, BookRole::Translator)
        .await
        .unwrap();
    members
        .add_collaborator(owner, a, moderator, BookRole::Moderator)
        .await
        .unwrap();
    assert!(
        repo.manageable_related_books(translator, b)
            .await
            .unwrap()
            .is_none(),
        "membership of Book A never grants Book B settings"
    );
    assert!(
        repo.manageable_related_books(translator, a)
            .await
            .unwrap()
            .is_some(),
        "active translator can open settings"
    );
    assert!(
        repo.manageable_related_books(moderator, a)
            .await
            .unwrap()
            .is_some(),
        "active book moderator can open settings"
    );

    assert!(repo
        .manageable_related_books(another, a)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.find_related_candidates(another, a, &b_title).await,
        Err(AuthorError::Missing)
    );
    let found = repo
        .find_related_candidates(owner, a, &b_title.to_uppercase())
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, b);
    // NEwBOO public-candidate invariant: a member can manage historical
    // relations but cannot discover or add a newly pending-deletion target.
    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .find_related_candidates(owner, a, &b_title)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        repo.add_related_book(owner, a, b).await,
        Err(AuthorError::Missing)
    );
    sqlx::query("UPDATE books SET lifecycle_status='in_progress' WHERE id=$1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .find_related_candidates(owner, a, &b_title)
        .await
        .unwrap()
        .iter()
        .any(|entry| entry.id == b));

    assert!(repo
        .find_related_candidates(owner, a, &private_title)
        .await
        .unwrap()
        .is_empty());
    assert!(repo
        .find_related_candidates(owner, a, &held_title)
        .await
        .unwrap()
        .is_empty());
    // NEwBOO: an authorized source-Book member may organize links to any
    // active public target, irrespective of the target age tier. This is
    // *management*, not an anonymous display/read grant.
    for (suffix, tier) in [
        ("adult", Some("adult")),
        ("pro", Some("pro_adult")),
        ("unknown", None),
    ] {
        let title = format!("S020 age-restricted related {mark} {suffix}");
        let target = make_book(
            &pool,
            format!("related:age:{suffix}:{mark}"),
            title.clone(),
            another,
            "public",
            false,
        )
        .await;
        sqlx::query("UPDATE books SET access_level=$1 WHERE id=$2")
            .bind(tier)
            .bind(target)
            .execute(&pool)
            .await
            .unwrap();
        let candidates = repo
            .find_related_candidates(owner, a, &title)
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1, "member search at {suffix}");
        assert_eq!(candidates[0].id, target);
        assert_eq!(repo.add_related_book(owner, a, target).await, Ok(()));
        assert!(repo
            .manageable_related_books(owner, a)
            .await
            .unwrap()
            .unwrap()
            .linked
            .iter()
            .any(|item| item.id == target));
        // Anonymous/public related-book cards remain basic-only. The
        // management link must never become a public age-tier bypass.
        assert!(!PgCatalog::new(pool.clone())
            .public_related_books(a)
            .await
            .unwrap()
            .iter()
            .any(|item| item.id == target));
        assert_eq!(repo.remove_related_book(owner, a, target).await, Ok(true));
    }
    assert_eq!(
        repo.add_related_book(owner, a, hidden).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.add_related_book(owner, a, held).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.add_related_book(another, a, b).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.add_related_book(owner, a, a).await,
        Err(AuthorError::Invalid)
    );
    // Two writers linking opposite directions concurrently cannot create
    // duplicate rows or wedge on opposing row locks.
    let (left, right) = tokio::join!(
        repo.add_related_book(owner, a, b),
        repo.add_related_book(another, b, a)
    );
    assert_eq!(left, Ok(()));
    assert_eq!(right, Ok(()));
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM book_related_links
         WHERE smaller_book_id=$1 AND larger_book_id=$2",
    )
    .bind(a.min(b))
    .bind(a.max(b))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "one canonical row is bilateral");
    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .manageable_related_books(owner, a)
        .await
        .unwrap()
        .unwrap()
        .linked
        .iter()
        .any(|entry| entry.id == b));
    assert_eq!(
        repo.add_related_book(owner, a, b).await,
        Err(AuthorError::Missing),
        "existing edge must not authorize a new addition during pending deletion",
    );
    sqlx::query("UPDATE books SET lifecycle_status='in_progress' WHERE id=$1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();

    // A pending-deletion Book is still editable by its active collaborators,
    // but its title and link must vanish from all public related-book cards.
    for hidden_book in [a, b] {
        sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
            .bind(hidden_book)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            PgCatalog::new(pool.clone())
                .public_related_books(a)
                .await
                .unwrap()
                .is_empty(),
            "pending-deletion source or target must be unlisted",
        );
        assert!(
            PgCatalog::new(pool.clone())
                .public_related_books(b)
                .await
                .unwrap()
                .is_empty(),
            "reverse public link must also be unlisted",
        );
        assert!(
            repo.manageable_related_books(owner, a)
                .await
                .unwrap()
                .unwrap()
                .linked
                .iter()
                .any(|item| item.id == b),
            "Book-member management must retain the canonical relation",
        );
        sqlx::query("UPDATE books SET lifecycle_status='in_progress' WHERE id=$1")
            .bind(hidden_book)
            .execute(&pool)
            .await
            .unwrap();
    }
    assert!(PgCatalog::new(pool.clone())
        .public_related_books(a)
        .await
        .unwrap()
        .iter()
        .any(|item| item.id == b));

    assert_eq!(
        repo.manageable_related_books(translator, a)
            .await
            .unwrap()
            .unwrap()
            .linked[0]
            .id,
        b
    );
    assert_eq!(
        repo.find_related_candidates(moderator, a, &b_title)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(repo.add_related_book(translator, a, b).await, Ok(()));
    assert_eq!(repo.remove_related_book(moderator, a, b).await, Ok(true));
    assert_eq!(repo.add_related_book(moderator, a, b).await, Ok(()));
    // Moving a linked Book to adult must hide public cards, without
    // deleting the link or revoking scoped Book-member management.
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .manageable_related_books(owner, a)
        .await
        .unwrap()
        .unwrap()
        .linked
        .iter()
        .any(|item| item.id == b));
    assert!(!PgCatalog::new(pool.clone())
        .public_related_books(a)
        .await
        .unwrap()
        .iter()
        .any(|item| item.id == b));
    assert_eq!(repo.add_related_book(owner, a, b).await, Ok(()));
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    // Changing and then revoking Book membership changes manage rights,
    // without altering bought content or requiring a fresh authentication.
    members
        .change_collaborator_role(owner, a, translator, BookRole::Moderator)
        .await
        .unwrap();
    assert!(repo
        .manageable_related_books(translator, a)
        .await
        .unwrap()
        .is_some());
    members
        .revoke_collaborator(owner, a, moderator)
        .await
        .unwrap();
    assert!(repo
        .manageable_related_books(moderator, a)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.find_related_candidates(moderator, a, &b_title).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.add_related_book(moderator, a, b).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.remove_related_book(moderator, a, b).await,
        Err(AuthorError::Missing)
    );
    let a_links = repo
        .manageable_related_books(owner, a)
        .await
        .unwrap()
        .unwrap();
    let b_links = repo
        .manageable_related_books(another, b)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a_links.linked.len(), 1);
    assert_eq!(a_links.linked[0].id, b);
    assert_eq!(b_links.linked.len(), 1);
    assert_eq!(b_links.linked[0].id, a);

    // Ordinary deletion leaves the historical edge (and the Book row)
    // available for older purchases; neither source nor target may be
    // rediscovered or newly relinked. Pending-deletion member management
    // remains covered separately above and must not be conflated with this.
    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',visibility='draft' WHERE id=$1",
    )
    .bind(b)
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo
        .find_related_candidates(owner, a, &b_title)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        repo.add_related_book(owner, a, b).await,
        Err(AuthorError::Missing)
    );
    assert!(repo
        .manageable_related_books(owner, a)
        .await
        .unwrap()
        .unwrap()
        .linked
        .is_empty());
    assert!(PgCatalog::new(pool.clone())
        .public_related_books(a)
        .await
        .unwrap()
        .is_empty());
    assert!(PgCatalog::new(pool.clone())
        .public_related_books(b)
        .await
        .unwrap()
        .is_empty());
    let historical_links: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM book_related_links
         WHERE smaller_book_id=$1 AND larger_book_id=$2",
    )
    .bind(a.min(b))
    .bind(a.max(b))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(historical_links, 1, "do not delete old relations");
    sqlx::query("UPDATE books SET lifecycle_status='in_progress',visibility='public' WHERE id=$1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    assert!(PgCatalog::new(pool.clone())
        .public_related_books(a)
        .await
        .unwrap()
        .iter()
        .any(|item| item.id == b));

    // A completed deletion of the source freezes member settings and link
    // writes even though is_deleted stays FALSE for immutable entitlements.
    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',visibility='draft' WHERE id=$1",
    )
    .bind(a)
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo
        .manageable_related_books(owner, a)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.find_related_candidates(owner, a, &b_title).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.add_related_book(owner, a, b).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.remove_related_book(owner, a, b).await,
        Err(AuthorError::Missing)
    );
    assert!(PgCatalog::new(pool.clone())
        .public_related_books(b)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("UPDATE books SET lifecycle_status='in_progress',visibility='public' WHERE id=$1")
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
    let still_linked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM book_related_links
         WHERE smaller_book_id=$1 AND larger_book_id=$2",
    )
    .bind(a.min(b))
    .bind(a.max(b))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(still_linked, 1);

    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .manageable_related_books(owner, a)
        .await
        .unwrap()
        .unwrap()
        .linked
        .is_empty());
    assert_eq!(
        repo.remove_related_book(another, a, b).await,
        Err(AuthorError::Missing)
    );
    // The owner can remove a formerly public link after the target hides.
    assert_eq!(repo.remove_related_book(owner, a, b).await, Ok(true));
    assert_eq!(repo.remove_related_book(owner, a, b).await, Ok(false));
    // A draft source remains editable by its active team, but an admin hold
    // removes everyone (including the owner) from this settings capability.
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .manageable_related_books(translator, a)
        .await
        .unwrap()
        .is_some());
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .manageable_related_books(translator, a)
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .manageable_related_books(owner, a)
        .await
        .unwrap()
        .is_none());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM book_related_links WHERE smaller_book_id=$1")
            .bind(a.min(b))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    pool.close().await;
}
