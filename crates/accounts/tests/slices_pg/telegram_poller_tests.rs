use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_07_telegram_poller_lease_contends_fences_and_recovers_after_expiry() {
    let (accounts, user, _) = fixture("tg-poller-lease").await;
    let bot_id = 1_000_000_000 + user;
    let owner_a = session_token_digest(&format!("poller-owner-a-{user}"));
    let owner_b = session_token_digest(&format!("poller-owner-b-{user}"));
    // Parallel contenders cannot both own the bot's getUpdates stream.
    let (a, b) = tokio::join!(
        accounts.claim_telegram_poller(bot_id, &owner_a),
        accounts.claim_telegram_poller(bot_id, &owner_b)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.is_some(), b.is_some());
    let (winner, loser, epoch) = if let Some(epoch) = a {
        (&owner_a, &owner_b, epoch)
    } else {
        (&owner_b, &owner_a, b.unwrap())
    };
    assert!(!accounts
        .renew_telegram_poller(bot_id, loser, epoch)
        .await
        .unwrap());
    assert!(accounts
        .renew_telegram_poller(bot_id, winner, epoch)
        .await
        .unwrap());
    let update_id = 3_000_000_000 + user;
    assert_eq!(
        accounts
            .acknowledge_telegram_update_fenced(bot_id, loser, epoch, update_id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        accounts
            .acknowledge_telegram_update_fenced(bot_id, winner, epoch, update_id)
            .await
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        accounts
            .acknowledge_telegram_update_fenced(bot_id, winner, epoch, update_id)
            .await
            .unwrap(),
        Some(false)
    );
    assert!(!accounts
        .release_telegram_poller(bot_id, loser, epoch)
        .await
        .unwrap());
    assert!(accounts
        .release_telegram_poller(bot_id, winner, epoch)
        .await
        .unwrap());
    assert_eq!(
        accounts
            .acknowledge_telegram_update_fenced(bot_id, winner, epoch, update_id + 1)
            .await
            .unwrap(),
        None
    );

    let next = accounts
        .claim_telegram_poller(bot_id, loser)
        .await
        .unwrap()
        .unwrap();
    assert!(next > epoch, "lease generations must never reset");
    assert!(!accounts
        .renew_telegram_poller(bot_id, winner, epoch)
        .await
        .unwrap());
    assert!(!accounts
        .release_telegram_poller(bot_id, winner, epoch)
        .await
        .unwrap());
    assert_eq!(
        accounts
            .acknowledge_telegram_update_fenced(bot_id, winner, epoch, update_id + 1)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        accounts
            .acknowledge_telegram_update_fenced(bot_id, loser, next, update_id + 1)
            .await
            .unwrap(),
        Some(true)
    );
    assert!(accounts
        .renew_telegram_poller(bot_id, loser, next)
        .await
        .unwrap());

    // Simulate a crashed/paused worker expiring. A stale renewal must fail.
    sqlx::query(
        "UPDATE account_telegram_poller_leases
         SET expires_at=clock_timestamp()-INTERVAL '1 second'
         WHERE bot_id=$1",
    )
    .bind(bot_id)
    .execute(accounts.pool())
    .await
    .unwrap();
    assert!(!accounts
        .renew_telegram_poller(bot_id, loser, next)
        .await
        .unwrap());
    let recovered = accounts
        .claim_telegram_poller(bot_id, winner)
        .await
        .unwrap()
        .unwrap();
    assert!(recovered > next);
    assert!(!accounts
        .release_telegram_poller(bot_id, loser, next)
        .await
        .unwrap());
    assert!(accounts
        .renew_telegram_poller(bot_id, winner, recovered)
        .await
        .unwrap());
    assert!(accounts
        .release_telegram_poller(bot_id, winner, recovered)
        .await
        .unwrap());
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_07_stale_worker_cannot_claim_link_or_ack_notification() {
    let (accounts, user, session) = fixture("tg-fenced-side-effects").await;
    let tg_id = 14_000_000 + user;
    let bot_id = 14_000_000_000 + user;
    let owner_a = session_token_digest(&format!("tg-fence-owner-a-{user}"));
    let owner_b = session_token_digest(&format!("tg-fence-owner-b-{user}"));
    let nonce = session_token_digest(&format!("tg-fenced-link-{user}"));
    assert!(accounts
        .begin_telegram_link(user, &session, "$argon2id$fixture", &nonce)
        .await
        .unwrap());
    let epoch_a = accounts
        .claim_telegram_poller(bot_id, &owner_a)
        .await
        .unwrap()
        .unwrap();
    let old = TelegramPollerFence {
        bot_id,
        owner_digest: &owner_a,
        lease_epoch: epoch_a,
    };
    let foreign = TelegramPollerFence {
        bot_id,
        owner_digest: &owner_b,
        lease_epoch: epoch_a,
    };
    assert_eq!(
        accounts
            .claim_telegram_link_fenced(foreign, &nonce, tg_id, tg_id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        accounts
            .claim_telegram_link_fenced(old, &nonce, tg_id, tg_id + 1)
            .await
            .unwrap(),
        Some(false)
    );
    assert!(accounts
        .release_telegram_poller(bot_id, &owner_a, epoch_a)
        .await
        .unwrap());
    let epoch_b = accounts
        .claim_telegram_poller(bot_id, &owner_b)
        .await
        .unwrap()
        .unwrap();
    assert!(epoch_b > epoch_a);
    assert_eq!(
        accounts
            .claim_telegram_link_fenced(old, &nonce, tg_id, tg_id)
            .await
            .unwrap(),
        None
    );
    let candidate: Option<i64> = sqlx::query_scalar(
        "SELECT candidate_telegram_user_id FROM account_telegram_link_intents
         WHERE user_id=$1",
    )
    .bind(user)
    .fetch_one(accounts.pool())
    .await
    .unwrap();
    assert_eq!(candidate, None, "stale bot must not change a link intent");
    let current = TelegramPollerFence {
        bot_id,
        owner_digest: &owner_b,
        lease_epoch: epoch_b,
    };
    assert_eq!(
        accounts
            .claim_telegram_link_fenced(current, &nonce, tg_id, tg_id)
            .await
            .unwrap(),
        Some(true)
    );
    let codes: [[u8; 32]; 8] =
        std::array::from_fn(|i| session_token_digest(&format!("tg-fenced-code-{user}-{i}")));
    assert_eq!(
        accounts.finish_telegram_link(user, &session, &codes).await.unwrap(),
        Some(tg_id)
    );
    let email: String = sqlx::query_scalar("SELECT email_normalized FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(accounts.pool())
        .await
        .unwrap();
    let browser = session_token_digest(&format!("tg-fenced-browser-{user}"));
    accounts
        .begin_browser_telegram_recovery(Some(&email), &browser, "Fnc_13")
        .await
        .unwrap();
    let request_id = accounts
        .pending_telegram_recovery_notifications()
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.private_chat_id == tg_id)
        .unwrap()
        .id;
    assert_eq!(
        accounts
            .mark_telegram_recovery_notified_fenced(old, request_id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        accounts
            .mark_telegram_recovery_notified_fenced(current, request_id)
            .await
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        accounts
            .mark_telegram_recovery_notified_fenced(current, request_id)
            .await
            .unwrap(),
        Some(false)
    );
    assert!(accounts
        .release_telegram_poller(bot_id, &owner_b, epoch_b)
        .await
        .unwrap());
}
