//! B-09a / S030: real PostgreSQL creator roles, no Team/read entitlements.
use sqlx::{PgPool, Row};
use tor_books::{
    repositories::{book_members::PgBookMembers, postgres::PgAuthoring, PgCatalog},
    services::authoring::{new_book, AuthorError},
};
use tor_contracts::access::BookRole;

async fn user(pool: &PgPool, email: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'B09 fixture','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE
           SET display_name=EXCLUDED.display_name
         RETURNING id",
    )
    .bind(email)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL TEST_DATABASE_URL ending /tor_ci"]
async fn b_09_pg_creator_membership_owner_scoped_revocation_and_holds() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG CI URL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();

    let owner = user(&pool, "b09-owner@tor-ci.example").await;
    let translator = user(&pool, "b09-translator@tor-ci.example").await;
    let outsider = user(&pool, "b09-outsider@tor-ci.example").await;

    let author = PgAuthoring::new(pool.clone());
    let book = new_book(
        &format!("ci:b09:creator:{owner}"),
        "ru",
        "B-09 Creator ACL",
        &"q".repeat(43),
    )
    .unwrap();
    let book_id = author.create_owned_book(owner, &book).await.unwrap();
    assert_eq!(
        author.create_owned_book(owner, &book).await.unwrap(),
        book_id,
        "idempotent retries cannot create additional owner members"
    );
    let members = PgBookMembers::new(pool.clone());
    assert_eq!(
        members.active_role(owner, book_id).await.unwrap(),
        Some(BookRole::Owner)
    );
    assert_eq!(members.active_role(outsider, book_id).await.unwrap(), None);
    let owner_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM book_members
         WHERE book_id=$1 AND role='owner' AND is_active",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(owner_rows, 1);

    assert_eq!(
        members
            .add_collaborator(outsider, book_id, translator, BookRole::Translator)
            .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        members
            .add_collaborator(owner, book_id, translator, BookRole::Owner)
            .await,
        Err(AuthorError::Forbidden)
    );
    members
        .add_collaborator(owner, book_id, translator, BookRole::Translator)
        .await
        .unwrap();
    assert_eq!(
        members.active_role(translator, book_id).await.unwrap(),
        Some(BookRole::Translator)
    );
    assert_eq!(
        members
            .add_collaborator(owner, book_id, translator, BookRole::Translator)
            .await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        members
            .change_collaborator_role(translator, book_id, outsider, BookRole::Moderator)
            .await,
        Err(AuthorError::Missing)
    );
    members
        .change_collaborator_role(owner, book_id, translator, BookRole::Moderator)
        .await
        .unwrap();
    assert_eq!(
        members.active_role(translator, book_id).await.unwrap(),
        Some(BookRole::Moderator)
    );
    assert_eq!(
        members.revoke_collaborator(owner, book_id, owner).await,
        Err(AuthorError::Conflict),
        "generic staff changes must never revoke the owner"
    );
    members
        .revoke_collaborator(owner, book_id, translator)
        .await
        .unwrap();
    assert_eq!(
        members.active_role(translator, book_id).await.unwrap(),
        None
    );
    members
        .add_collaborator(owner, book_id, translator, BookRole::Translator)
        .await
        .unwrap();
    let active_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM book_members
         WHERE book_id=$1 AND user_id=$2 AND is_active",
    )
    .bind(book_id)
    .bind(translator)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active_rows, 1, "revoked users reactivate, never duplicate");

    // A member with a revenue share cannot be silently dropped by the staff
    // management operation; economic obligations are a separate B-10 boundary.
    sqlx::query("UPDATE book_members SET share_percent=25 WHERE book_id=$1 AND user_id=$2")
        .bind(book_id)
        .bind(translator)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        members
            .revoke_collaborator(owner, book_id, translator)
            .await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        members.active_role(translator, book_id).await.unwrap(),
        Some(BookRole::Translator)
    );
    sqlx::query("UPDATE book_members SET share_percent=0 WHERE book_id=$1 AND user_id=$2")
        .bind(book_id)
        .bind(translator)
        .execute(&pool)
        .await
        .unwrap();

    // The AFTER INSERT trigger covers other B-authorized SQL book creators,
    // not just PgAuthoring's command path.
    let raw_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','B09 raw owner fixture','draft',$2)
         ON CONFLICT(source_identity,target_language)
         DO UPDATE SET title=EXCLUDED.title RETURNING id",
    )
    .bind(format!("ci:b09:raw:{owner}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        members.active_role(owner, raw_book).await.unwrap(),
        Some(BookRole::Owner)
    );
    assert_eq!(
        members.active_role(translator, raw_book).await.unwrap(),
        None,
        "book roles do not leak across Books or from Team membership"
    );

    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(members.active_role(owner, book_id).await.unwrap(), None);
    assert_eq!(
        members.active_role(translator, book_id).await.unwrap(),
        None
    );
    assert_eq!(
        members
            .revoke_collaborator(owner, book_id, translator)
            .await,
        Err(AuthorError::Missing),
        "hard-held books do not permit role mutation"
    );

    let role: String = sqlx::query("SELECT role FROM book_members WHERE book_id=$1 AND user_id=$2")
        .bind(book_id)
        .bind(translator)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("role");
    assert_eq!(role, "translator", "hold must not silently revoke roles");
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL TEST_DATABASE_URL ending /tor_ci"]
async fn b_09_e_08_private_mail_title_checks_live_owner_membership() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG CI URL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();
    let owner = user(&pool, "b09-private-mail-owner@tor-ci.example").await;
    let outsider = user(&pool, "b09-private-mail-outsider@tor-ci.example").await;
    let author = PgAuthoring::new(pool.clone());
    let book = new_book(
        &format!("ci:b09:private-mail:{owner}"),
        "ru",
        "Private mail title",
        &"m".repeat(43),
    )
    .unwrap();
    let book_id = author.create_owned_book(owner, &book).await.unwrap();
    let catalog = PgCatalog::new(pool.clone());
    assert_eq!(
        catalog
            .private_mail_title_for_owner(owner, book_id)
            .await
            .unwrap(),
        Some("Private mail title".to_owned()),
    );
    assert_eq!(
        catalog
            .private_mail_title_for_owner(outsider, book_id)
            .await
            .unwrap(),
        None,
    );
    assert_eq!(
        catalog
            .private_mail_title_for_owner(0, book_id)
            .await
            .unwrap(),
        None,
    );

    // B-10 protects the 100% current revenue recipient against a bare
    // membership deactivation. This security test must not bypass finance.
    let denied =
        sqlx::query("UPDATE book_members SET is_active=FALSE WHERE book_id=$1 AND user_id=$2")
            .bind(book_id)
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap_err();
    assert_eq!(
        denied
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514"),
    );

    // Even an inconsistent owner role left by a legacy mutation must not
    // expose a private title just because books.owner_user_id still matches.
    sqlx::query("UPDATE book_members SET role='translator' WHERE book_id=$1 AND user_id=$2")
        .bind(book_id)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        catalog
            .private_mail_title_for_owner(owner, book_id)
            .await
            .unwrap(),
        None,
    );
    sqlx::query("UPDATE book_members SET role='owner' WHERE book_id=$1 AND user_id=$2")
        .bind(book_id)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        catalog
            .private_mail_title_for_owner(owner, book_id)
            .await
            .unwrap(),
        Some("Private mail title".to_owned()),
    );
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        catalog
            .private_mail_title_for_owner(owner, book_id)
            .await
            .unwrap(),
        None,
    );
    pool.close().await;
}
