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
