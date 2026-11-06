//! S015: a durable notification must never become a durable read grant.
use sqlx::PgPool;
use tor_social::{
    repositories::{mail::PgMail, notifications::PgNotifications},
    services::MailService,
};

async fn hidden(pool: &PgPool, repo: &PgNotifications, reader: i64, notice: i64) {
    for section in ["notifications", "releases"] {
        assert!(repo
            .list_section(reader, None, 50, section)
            .await
            .unwrap()
            .is_empty());
    }
    assert!(!repo.mark_read(reader, notice).await.unwrap());
    assert_eq!(repo.mark_all_read(reader).await.unwrap(), 0);
    let unread: bool =
        sqlx::query_scalar("SELECT read_at IS NULL FROM user_notifications WHERE id=$1")
            .bind(notice)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(unread, "stale forms cannot acknowledge a hidden release");
}

pub(super) async fn verify(
    pool: &PgPool,
    repo: &PgNotifications,
    users: &[i64],
    book: i64,
    chapter: i64,
) {
    let (author, reader, outsider, other_reader) = (users[0], users[1], users[3], users[4]);
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    let initial = repo.list(reader, None, 50).await.unwrap();
    let notice = initial[0].id;
    assert!(initial.iter().all(|n| n.kind == "chapter_created"));
    assert!(!repo.mark_read(outsider, notice).await.unwrap());
    let mail = MailService::new(PgMail::new(pool.clone()));

    for (blocker, blocked) in [(author, reader), (reader, author)] {
        mail.set_block(blocker, blocked, true).await.unwrap();
        hidden(pool, repo, reader, notice).await;
        assert!(!repo.list(other_reader, None, 50).await.unwrap().is_empty());
        mail.set_block(blocker, blocked, false).await.unwrap();
        assert_eq!(
            repo.list(reader, None, 50).await.unwrap().len(),
            initial.len()
        );
    }

    for (hide, restore) in [
        (
            "UPDATE books SET visibility='draft' WHERE id=$1",
            "UPDATE books SET visibility='public' WHERE id=$1",
        ),
        (
            "UPDATE books SET visibility_status='hidden' WHERE id=$1",
            "UPDATE books SET visibility_status='active' WHERE id=$1",
        ),
        (
            "UPDATE books SET admin_hold=true WHERE id=$1",
            "UPDATE books SET admin_hold=false WHERE id=$1",
        ),
        (
            "UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1",
            "UPDATE books SET lifecycle_status='in_progress' WHERE id=$1",
        ),
        (
            "UPDATE books SET access_level='adult' WHERE id=$1",
            "UPDATE books SET access_level='basic' WHERE id=$1",
        ),
    ] {
        sqlx::query("UPDATE users SET adult_enabled=false WHERE id=$1")
            .bind(reader)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(hide).bind(book).execute(pool).await.unwrap();
        hidden(pool, repo, reader, notice).await;
        sqlx::query(restore).bind(book).execute(pool).await.unwrap();
        assert_eq!(
            repo.list(reader, None, 50).await.unwrap().len(),
            initial.len()
        );
    }
    for (hide, restore) in [
        (
            "UPDATE chapters SET visibility='draft' WHERE id=$1",
            "UPDATE chapters SET visibility='public' WHERE id=$1",
        ),
        (
            "UPDATE chapters SET admin_hold=true WHERE id=$1",
            "UPDATE chapters SET admin_hold=false WHERE id=$1",
        ),
        (
            "UPDATE chapters SET reader_released_at=clock_timestamp()+interval '1 day' WHERE id=$1",
            "UPDATE chapters SET reader_released_at=NULL WHERE id=$1",
        ),
        (
            "UPDATE chapters SET access_level='paid',price_amount=100 WHERE id=$1",
            "UPDATE chapters SET access_level='free',price_amount=0 WHERE id=$1",
        ),
    ] {
        sqlx::query(hide).bind(chapter).execute(pool).await.unwrap();
        hidden(pool, repo, reader, notice).await;
        sqlx::query(restore)
            .bind(chapter)
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    hidden(pool, repo, reader, notice).await;
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',true)",
    )
    .bind(reader)
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(
        repo.list(reader, None, 50).await.unwrap().len(),
        initial.len()
    );
    sqlx::query("UPDATE user_level_grants SET is_active=false WHERE user_id=$1")
        .bind(reader)
        .execute(pool)
        .await
        .unwrap();
    hidden(pool, repo, reader, notice).await;
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();

    // A visible older mail notification must fill the page even when newer
    // chapter notices become unavailable. Filtering after LIMIT loses it.
    let conversation = mail.start(author, reader).await.unwrap();
    mail.send(author, conversation, "private body", &"p".repeat(43))
        .await
        .unwrap();
    repo.deliver_pending(100).await.unwrap();
    sqlx::query(
        "INSERT INTO outbox_events(
             event_type,schema_version,aggregate_kind,aggregate_id,payload,required_consumers)
         SELECT 'reader.chapter.released',1,'chapter',$1,
                jsonb_build_object('actor_user_id',$2,'recipient_user_id',$3,
                                   'object_id',$1,'book_id',$4,'version_id',$5),
                ARRAY['user_notifications']::text[]
         FROM generate_series(1,3)",
    )
    .bind(chapter)
    .bind(author)
    .bind(reader)
    .bind(book)
    .bind(
        sqlx::query_scalar::<_, i64>(
            "SELECT version_id FROM chapter_publications WHERE chapter_id=$1",
        )
        .bind(chapter)
        .fetch_one(pool)
        .await
        .unwrap(),
    )
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(repo.deliver_pending(100).await.unwrap(), 3);
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    let page = repo.list(reader, None, 1).await.unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].kind, "mail");
    assert!(repo
        .list_section(reader, None, 1, "releases")
        .await
        .unwrap()
        .is_empty());
    assert!(!repo.mark_read(reader, notice).await.unwrap());
    assert_eq!(repo.mark_all_read(reader).await.unwrap(), 1);
    sqlx::query("UPDATE books SET visibility='public' WHERE id=$1")
        .bind(book)
        .execute(pool)
        .await
        .unwrap();
    let restored = repo.list(reader, None, 50).await.unwrap();
    assert_eq!(restored.len(), initial.len() + 4);
    assert!(restored
        .iter()
        .filter(|n| n.kind == "chapter_created")
        .all(|n| !n.is_read));
    assert!(repo.mark_read(reader, notice).await.unwrap());
}
