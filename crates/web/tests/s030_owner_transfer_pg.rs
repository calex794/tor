//! S030: real A-issued one-time step-up, PostgreSQL Book transfer and finance
//! without production migrations, API-key fixtures or mocked ACL decisions.
#![forbid(unsafe_code)]
use sqlx::PgPool;
use tor_accounts::{
    repositories::{
        book_step_up::{BOOK_SHARES_SCOPE, BOOK_TRANSFER_SCOPE},
        PgAccounts,
    },
    services::security::{hash_password, session_token_digest, verify_password_for_step_up},
};
use tor_books::{
    domain::shares::ShareAssignment,
    repositories::{
        owner_transfer::{owner_transfer_payload_digest, PgOwnerTransfer},
        revenue_shares::{revenue_share_payload_digest, PgRevenueShares},
        translation_team_books::PgTeamBooks,
    },
    services::authoring::AuthorError,
};

fn password() -> &'static str {
    "S030SecureTransferPassword2026"
}

async fn pool() -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG CI url");
    assert!(url.contains("127.0.0.1:5432") && url.ends_with("/tor_ci"));
    PgPool::connect(&url).await.expect("isolated PG")
}

async fn user(pool: &PgPool, key: &str, name: &str, hash: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,$2,$3) RETURNING id",
    )
    .bind(format!("s030-{key}@tor-ci.example"))
    .bind(name)
    .bind(hash)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn book(pool: &PgPool, key: &str, owner: i64, level: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,
                           owner_user_id,access_level)
         VALUES($1,'ru','S030 book','draft',$2,$3) RETURNING id",
    )
    .bind(format!("ci:s030:{key}:{owner}"))
    .bind(owner)
    .bind(level)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn session(pool: &PgPool, actor: i64, name: &str) -> [u8; 32] {
    let digest = session_token_digest(name);
    sqlx::query(
        "INSERT INTO sessions(token_hash,user_id,expires_at)
         VALUES($1,$2,now()+INTERVAL '1 hour')",
    )
    .bind(digest.as_slice())
    .bind(actor)
    .execute(pool)
    .await
    .unwrap();
    digest
}

async fn transfer_token(
    accounts: &PgAccounts,
    actor: i64,
    book_id: i64,
    name: &str,
    digest: &[u8; 32],
    hash: &str,
) -> String {
    let proof = verify_password_for_step_up(password(), hash).unwrap();
    let payload = owner_transfer_payload_digest(actor, book_id, actor, name);
    accounts
        .issue_book_mutation_step_up(
            actor,
            digest,
            &proof,
            book_id,
            actor,
            BOOK_TRANSFER_SCOPE,
            &payload,
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL"]
async fn s030_pg_one_owner_team_continuity_immutable_revenue_and_replay() {
    let pool = pool().await;
    let hash = hash_password(password()).unwrap();
    let from = user(&pool, "team-owner", "S030 Team Owner", &hash).await;
    let to = user(&pool, "team-recipient", "S030 Team Recipient", &hash).await;
    let digest = session(&pool, from, "s030-team-owner-session").await;
    let recipient_digest = session(&pool, to, "s030-team-recipient-session").await;
    let book_id = book(&pool, "team-continuity", from, "basic").await;

    let team: i64 = sqlx::query_scalar(
        "INSERT INTO translation_teams(owner_user_id,name)
         VALUES($1,'S030 team transfer') RETURNING id",
    )
    .bind(from)
    .fetch_one(&pool)
    .await
    .unwrap();
    let invite = session_token_digest("s030-team-invite");
    sqlx::query(
        "INSERT INTO translation_team_invites
         (token_digest,team_id,invitee_user_id,inviter_user_id,status,decided_at)
         VALUES($1,$2,$3,$4,'accepted',now())",
    )
    .bind(invite.as_slice())
    .bind(team)
    .bind(to)
    .bind(from)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO translation_team_members(team_id,user_id,invite_digest)
         VALUES($1,$2,$3)",
    )
    .bind(team)
    .bind(to)
    .bind(invite.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO translation_team_books(team_id,book_id,added_by_user_id)
         VALUES($1,$2,$3)",
    )
    .bind(team)
    .bind(book_id)
    .bind(from)
    .execute(&pool)
    .await
    .unwrap();

    let accounts = PgAccounts::from_pool(pool.clone());
    let repo = PgOwnerTransfer::new(pool.clone());
    assert!(repo.settings(from, book_id).await.unwrap().is_some());
    assert!(repo.settings(to, book_id).await.unwrap().is_none());
    let token = transfer_token(
        &accounts,
        from,
        book_id,
        "S030 Team Recipient",
        &digest,
        &hash,
    )
    .await;
    repo.transfer(from, book_id, from, "S030 Team Recipient", &digest, &token)
        .await
        .unwrap();

    let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owner, to);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM book_members
         WHERE book_id=$1 AND is_active AND role='owner'",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM book_owner_transfer_events
         WHERE book_id=$1 AND previous_owner_user_id=$2
           AND new_owner_user_id=$3 AND share_revision=1",
    )
    .bind(book_id)
    .bind(from)
    .bind(to)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 1);
    assert!(repo.settings(from, book_id).await.unwrap().is_none());
    assert!(repo.settings(to, book_id).await.unwrap().is_some());
    let linked = PgTeamBooks::new(pool.clone()).view(to, team).await.unwrap();
    assert!(linked.linked.iter().any(|row| row.id == book_id));
    assert_eq!(
        repo.transfer(from, book_id, from, "S030 Team Recipient", &digest, &token)
            .await,
        Err(AuthorError::Forbidden),
        "consumed proof must never be replayed",
    );

    let assignments = [
        ShareAssignment {
            user_id: from,
            share_percent: 65,
        },
        ShareAssignment {
            user_id: to,
            share_percent: 35,
        },
    ];
    let reason = "new owner distributes creator pool";
    let schedule_digest =
        revenue_share_payload_digest(to, book_id, 1, &assignments, reason).unwrap();
    let verified = verify_password_for_step_up(password(), &hash).unwrap();
    let share_token = accounts
        .issue_book_mutation_step_up(
            to,
            &recipient_digest,
            &verified,
            book_id,
            1,
            BOOK_SHARES_SCOPE,
            &schedule_digest,
        )
        .await
        .unwrap();
    let revenue = PgRevenueShares::new(pool.clone());
    assert_eq!(
        revenue
            .replace_schedule_verified(
                to,
                book_id,
                1,
                &assignments,
                reason,
                &recipient_digest,
                &share_token,
            )
            .await
            .unwrap(),
        2,
    );
    let old: i16 = sqlx::query_scalar(
        "SELECT share_percent FROM revenue_share_assignments
         WHERE book_id=$1 AND revision=1 AND user_id=$2",
    )
    .bind(book_id)
    .bind(from)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(old, 100, "old purchase revision must remain unchanged");
    assert!(
        revenue
            .replace_schedule_verified(
                to,
                book_id,
                2,
                &assignments,
                reason,
                &recipient_digest,
                &share_token,
            )
            .await
            .is_err(),
        "one-time proof must be consumed"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL"]
async fn s030_pg_denies_age_layer_history_revoked_session_and_forger() {
    let pool = pool().await;
    let hash = hash_password(password()).unwrap();
    let from = user(&pool, "denied-owner", "S030 Denied Owner", &hash).await;
    let to = user(&pool, "denied-recipient", "S030 Denied Recipient", &hash).await;
    let digest = session(&pool, from, "s030-denied-owner-session").await;
    let book_id = book(&pool, "adult-layer", from, "adult").await;
    let accounts = PgAccounts::from_pool(pool.clone());
    let repo = PgOwnerTransfer::new(pool.clone());
    let age_token = transfer_token(
        &accounts,
        from,
        book_id,
        "S030 Denied Recipient",
        &digest,
        &hash,
    )
    .await;
    assert_eq!(
        repo.transfer(
            from,
            book_id,
            from,
            "S030 Denied Recipient",
            &digest,
            &age_token
        )
        .await,
        Err(AuthorError::Forbidden),
    );
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(to)
        .execute(&pool)
        .await
        .unwrap();

    // Even with no active subscribers, historic composition can be renewed.
    let layer: i64 = sqlx::query_scalar(
        "INSERT INTO commerce_layers(owner_user_id,title)
         VALUES($1,'S030 Historical Layer') RETURNING id",
    )
    .bind(from)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO commerce_layer_versions(layer_id,version_no)
         VALUES($1,1) RETURNING id",
    )
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE commerce_layers SET current_version_id=$2 WHERE id=$1")
        .bind(layer)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO commerce_layer_version_books(layer_version_id,book_id)
         VALUES($1,$2)",
    )
    .bind(version)
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    let layer_token = transfer_token(
        &accounts,
        from,
        book_id,
        "S030 Denied Recipient",
        &digest,
        &hash,
    )
    .await;
    assert_eq!(
        repo.transfer(
            from,
            book_id,
            from,
            "S030 Denied Recipient",
            &digest,
            &layer_token
        )
        .await,
        Err(AuthorError::Conflict),
    );
    let token = transfer_token(
        &accounts,
        from,
        book_id,
        "S030 Denied Recipient",
        &digest,
        &hash,
    )
    .await;
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(digest.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.transfer(
            from,
            book_id,
            from,
            "S030 Denied Recipient",
            &digest,
            &token
        )
        .await,
        Err(AuthorError::Forbidden),
    );
    let owned: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owned, from);
    let events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM book_owner_transfer_events WHERE book_id=$1")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events, 0);
    pool.close().await;
}
