//! E-02: a narrow, real PostgreSQL test for public book comments only.
use tor_discussions::repositories::public_book_comments::{CreateResult, PgPublicBookComments};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn e_02_public_book_visibility_and_replay() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PG URL");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users (email_normalized,display_name,password_hash)
         VALUES ('e02-public-comment@example.invalid','E02','$argon2id$fixture') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:e02:comment','ru','E02 Book','public') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgPublicBookComments::new(pool.clone());
    let command_key = "k".repeat(43);
    let hash = [3_u8; 32];
    let body = "Первый ||секрет @hidden_user|| после";
    let first = repo.create(user, book, body, &command_key, &hash).await;
    let id = match first {
        CreateResult::Created(id) => id,
        other => panic!("first write was {other:?}"),
    };
    assert_eq!(
        repo.create(user, book, body, &command_key, &hash).await,
        CreateResult::Replayed(id)
    );
    assert_eq!(
        repo.create(user, book, "Чужой текст", &command_key, &hash)
            .await,
        CreateResult::Conflict
    );
    let comments = repo.list(book, None, 50).await.unwrap().unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, id);
    assert_eq!(comments[0].body, "Первый ||секрет @hidden_user|| после");
    assert_eq!(
        tor_discussions::domain::spoilers::delivery_preview(&comments[0].body, 240),
        "Первый [спойлер] после"
    );
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(repo.list(book, None, 50).await.unwrap(), None);
    assert_eq!(
        repo.create(user, book, "Нельзя", &"n".repeat(43), &[4_u8; 32])
            .await,
        CreateResult::NotFound
    );
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn e_02_e03_replies_bind_parent_target_and_concurrent_replay() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PG URL");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('e03-replies@example.invalid','E03','$argon2id$fixture') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:e03:replies','ru','Replies','public') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let other_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:e03:other','ru','Other','public') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgPublicBookComments::new(pool.clone());
    let parent = match repo
        .create(user, book, "Parent", &"a".repeat(43), &[1; 32])
        .await
    {
        CreateResult::Created(id) => id,
        other => panic!("{other:?}"),
    };
    let key = "b".repeat(43);
    let (a, b) = tokio::join!(
        repo.create_with_parent(user, book, "Reply", &key, &[2; 32], Some(parent)),
        repo.create_with_parent(user, book, "Reply", &key, &[2; 32], Some(parent))
    );
    let id = match (&a, &b) {
        (CreateResult::Created(id), CreateResult::Replayed(replay))
        | (CreateResult::Replayed(replay), CreateResult::Created(id))
            if id == replay =>
        {
            *id
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(
        repo.create(user, book, "Reply", &key, &[2; 32]).await,
        CreateResult::Conflict
    );
    assert_eq!(
        repo.create_with_parent(
            user,
            other_book,
            "No",
            &"c".repeat(43),
            &[3; 32],
            Some(parent)
        )
        .await,
        CreateResult::NotFound
    );
    assert_eq!(
        repo.create_with_parent(user, book, "No", &"c".repeat(43), &[3; 32], Some(i64::MAX))
            .await,
        CreateResult::NotFound
    );
    let listed = repo.list(book, None, 25).await.unwrap().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].parent_id, Some(parent));
    assert_eq!(listed[0].id, id);
    // Replies can themselves receive replies; no parallel thread store.
    assert!(matches!(
        repo.create_with_parent(user, book, "Nested", &"d".repeat(43), &[4; 32], Some(id))
            .await,
        CreateResult::Created(_)
    ));
    for visibility in ["hidden", "deleted", "review"] {
        sqlx::query("UPDATE discussion_comments SET visibility=$1 WHERE id=$2")
            .bind(visibility)
            .bind(parent)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            repo.create_with_parent(user, book, "No", &"e".repeat(43), &[5; 32], Some(parent))
                .await,
            CreateResult::NotFound
        );
    }
    // A writer waiting for a moderation transaction must recheck its committed state.
    sqlx::query("UPDATE discussion_comments SET visibility='visible' WHERE id=$1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();
    let mut moderation = pool.begin().await.unwrap();
    sqlx::query("UPDATE discussion_comments SET visibility='hidden' WHERE id=$1")
        .bind(parent)
        .execute(&mut *moderation)
        .await
        .unwrap();
    let writer_repo = repo.clone();
    let writer = tokio::spawn(async move {
        writer_repo
            .create_with_parent(
                user,
                book,
                "Racing",
                &"f".repeat(43),
                &[6; 32],
                Some(parent),
            )
            .await
    });
    moderation.commit().await.unwrap();
    assert_eq!(writer.await.unwrap(), CreateResult::NotFound);
    // Delete deepest children first: canonical parent FK remains intact.
    sqlx::query("DELETE FROM discussion_comments WHERE parent_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM discussion_comments WHERE parent_id=$1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id IN ($1,$2)")
        .bind(book)
        .bind(other_book)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn e_02_e03_received_replies_are_private_live_references() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PG URL");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let users: Vec<i64> = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) VALUES
         ('e03-inbox-owner@example.invalid','Owner','$argon2id$fixture'),
         ('e03-inbox-writer@example.invalid','Writer','$argon2id$fixture'),
         ('e03-inbox-other@example.invalid','Other','$argon2id$fixture') RETURNING id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:e03:inbox','ru','Reply book','public') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgPublicBookComments::new(pool.clone());
    let parent = match repo
        .create(
            users[0],
            book,
            "Private parent text",
            &"a".repeat(43),
            &[1; 32],
        )
        .await
    {
        CreateResult::Created(id) => id,
        other => panic!("{other:?}"),
    };
    let reply = match repo
        .create_with_parent(
            users[1],
            book,
            "||Private spoiler||",
            &"b".repeat(43),
            &[2; 32],
            Some(parent),
        )
        .await
    {
        CreateResult::Created(id) => id,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        repo.create_with_parent(
            users[1],
            book,
            "||Private spoiler||",
            &"b".repeat(43),
            &[2; 32],
            Some(parent)
        )
        .await,
        CreateResult::Replayed(reply)
    );
    assert!(matches!(
        repo.create_with_parent(
            users[0],
            book,
            "Self",
            &"c".repeat(43),
            &[3; 32],
            Some(parent)
        )
        .await,
        CreateResult::Created(_)
    ));
    let received = repo.received_replies(users[0], 10).await.unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].comment_id, reply);
    assert_eq!(received[0].book_id, book);
    assert_eq!(received[0].book_title, "Reply book");
    assert!(!received[0].is_read);
    use tor_discussions::repositories::public_book_comments::ReadStateResult;
    assert_eq!(
        repo.set_reply_read_state(users[1], reply, true).await,
        ReadStateResult::NotFound
    );
    assert_eq!(
        repo.set_reply_read_state(users[0], reply, true).await,
        ReadStateResult::Updated
    );
    assert_eq!(
        repo.set_reply_read_state(users[0], reply, true).await,
        ReadStateResult::Updated
    );
    assert!(repo.received_replies(users[0], 10).await.unwrap()[0].is_read);
    assert_eq!(
        repo.set_reply_read_state(users[0], reply, false).await,
        ReadStateResult::Updated
    );
    assert_eq!(
        repo.set_reply_read_state(users[0], reply, false).await,
        ReadStateResult::Updated
    );
    assert!(!repo.received_replies(users[0], 10).await.unwrap()[0].is_read);

    assert!(repo
        .received_replies(users[1], 10)
        .await
        .unwrap()
        .is_empty());
    assert!(repo
        .received_replies(users[2], 10)
        .await
        .unwrap()
        .is_empty());
    assert!(repo.received_replies(0, 10).await.is_err());
    assert!(repo.received_replies(users[0], 101).await.is_err());
    assert!(repo.received_replies(users[0], 0).await.is_err());
    // Recreate repository, as after reconnect/restart: no in-memory delivery state.
    assert_eq!(
        PgPublicBookComments::new(pool.clone())
            .received_replies(users[0], 10)
            .await
            .unwrap(),
        received
    );
    for id in [parent, reply] {
        for state in ["hidden", "deleted", "review"] {
            sqlx::query("UPDATE discussion_comments SET visibility=$1 WHERE id=$2")
                .bind(state)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            assert!(repo
                .received_replies(users[0], 10)
                .await
                .unwrap()
                .is_empty());
        }
        sqlx::query("UPDATE discussion_comments SET visibility='visible' WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    for change in ["admin_hold=true", "is_deleted=true", "visibility='draft'"] {
        sqlx::query(&format!("UPDATE books SET {change} WHERE id=$1"))
            .bind(book)
            .execute(&pool)
            .await
            .unwrap();
        assert!(repo
            .received_replies(users[0], 10)
            .await
            .unwrap()
            .is_empty());
        sqlx::query(
            "UPDATE books SET admin_hold=false,is_deleted=false,visibility='public' WHERE id=$1",
        )
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    }
    let second = match repo
        .create_with_parent(
            users[1],
            book,
            "Second",
            &"d".repeat(43),
            &[4; 32],
            Some(parent),
        )
        .await
    {
        CreateResult::Created(id) => id,
        other => panic!("{other:?}"),
    };
    let bounded = repo.received_replies(users[0], 1).await.unwrap();
    assert_eq!(bounded.len(), 1);
    assert_eq!(bounded[0].comment_id, second);
    sqlx::query("DELETE FROM discussion_comments WHERE parent_id=$1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM discussion_comments WHERE id=$1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=ANY($1)")
        .bind(&users)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
}
