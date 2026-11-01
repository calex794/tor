//! E-08: isolated PostgreSQL + HTTP regression for consent-based creator handoff.
use super::{call, pg, text};
use axum::{http::StatusCode, Router};

pub(super) async fn check_consent(
    app: &Router,
    url: &str,
    path: &str,
    group_id: i64,
    users: &[i64],
    cookies: &[String],
    tokens: &[String],
) {
    // Only the creator can propose management. The roles stay unchanged until
    // the named recipient explicitly accepts; old offers are single-use.
    let transfer_path = format!("{path}/transfer");
    let transfer_to = |id: i64, token: &str| format!("new_creator_id={id}&csrf_token={token}");
    assert_eq!(
        call(
            app,
            "POST",
            &transfer_path,
            &cookies[0],
            &transfer_to(users[2], "invalid"),
            &[],
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            app,
            "POST",
            &transfer_path,
            &cookies[2],
            &transfer_to(users[0], &tokens[2]),
            &[],
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            app,
            "POST",
            &transfer_path,
            &cookies[3],
            &transfer_to(users[2], &tokens[3]),
            &[],
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    for invalid_target in [users[1], users[3]] {
        assert_eq!(
            call(
                app,
                "POST",
                &transfer_path,
                &cookies[0],
                &transfer_to(invalid_target, &tokens[0]),
                &[],
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        call(
            app,
            "POST",
            &transfer_path,
            &cookies[0],
            &transfer_to(users[0], &tokens[0]),
            &[],
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(
            app,
            "POST",
            &transfer_path,
            &cookies[0],
            &transfer_to(users[2], &tokens[0]),
            &[],
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let first_offer: i64 = pg(
        url,
        &format!(
            "SELECT id FROM social_group_creator_transfers WHERE group_id={group_id} AND state='pending'",
        ),
    )
    .await
    .parse()
    .unwrap();
    assert_eq!(
        call(
            app,
            "POST",
            &transfer_path,
            &cookies[0],
            &transfer_to(users[2], &tokens[0]),
            &[],
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let owner_before = text(call(app, "GET", path, &cookies[0], "", &[]).await).await;
    assert!(owner_before.contains("Предложить управление"));
    let recipient_before = text(call(app, "GET", path, &cookies[2], "", &[]).await).await;
    assert!(recipient_before.contains(&format!("action=\"{path}/leave\"")));
    assert!(!recipient_before.contains(&format!("action=\"{path}/transfer\"")));
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT user_id FROM social_group_members WHERE group_id={group_id} AND role='creator' AND status='active'"
            ),
        )
        .await,
        users[0].to_string(),
    );

    let transfers = "/mail/group-transfers";
    let incoming = text(call(app, "GET", transfers, &cookies[2], "", &[]).await).await;
    assert!(incoming.contains("Принять управление"));
    assert!(incoming.contains("Отказаться"));
    assert!(incoming.contains("Team &lt;script&gt;"));
    assert!(!incoming.contains("Team <script>"));
    let outbox = text(call(app, "GET", transfers, &cookies[0], "", &[]).await).await;
    assert!(outbox.contains("Отозвать предложение"));
    let accept = |id: i64| format!("/mail/group-transfers/{id}/accept");
    let decline = |id: i64| format!("/mail/group-transfers/{id}/decline");
    let revoke = |id: i64| format!("{path}/transfer/{id}/revoke");
    assert_eq!(
        call(
            app,
            "POST",
            &accept(first_offer),
            &cookies[0],
            &format!("csrf_token={}", tokens[0]),
            &[],
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            app,
            "POST",
            &accept(first_offer),
            &cookies[2],
            "csrf_token=invalid",
            &[],
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            app,
            "POST",
            &revoke(first_offer),
            &cookies[2],
            &format!("csrf_token={}", tokens[2]),
            &[],
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            app,
            "POST",
            &decline(first_offer),
            &cookies[2],
            &format!("csrf_token={}", tokens[2]),
            &[],
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(
            app,
            "POST",
            &accept(first_offer),
            &cookies[2],
            &format!("csrf_token={}", tokens[2]),
            &[],
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            app,
            "POST",
            &transfer_path,
            &cookies[0],
            &transfer_to(users[2], &tokens[0]),
            &[],
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let second_offer: i64 = pg(
        url,
        &format!(
            "SELECT id FROM social_group_creator_transfers WHERE group_id={group_id} AND state='pending'",
        ),
    )
    .await
    .parse()
    .unwrap();
    assert!(second_offer > first_offer);
    assert_eq!(
        call(
            app,
            "POST",
            &revoke(second_offer),
            &cookies[0],
            &format!("csrf_token={}", tokens[0]),
            &[],
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(
            app,
            "POST",
            &accept(second_offer),
            &cookies[2],
            &format!("csrf_token={}", tokens[2]),
            &[],
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            app,
            "POST",
            &transfer_path,
            &cookies[0],
            &transfer_to(users[2], &tokens[0]),
            &[],
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let third_offer: i64 = pg(
        url,
        &format!(
            "SELECT id FROM social_group_creator_transfers WHERE group_id={group_id} AND state='pending'",
        ),
    )
    .await
    .parse()
    .unwrap();
    assert_eq!(
        call(
            app,
            "POST",
            &accept(third_offer),
            &cookies[2],
            &format!("csrf_token={}", tokens[2]),
            &[],
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(
            app,
            "POST",
            &accept(third_offer),
            &cookies[2],
            &format!("csrf_token={}", tokens[2]),
            &[],
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let previous_creator = text(call(app, "GET", path, &cookies[0], "", &[]).await).await;
    assert!(previous_creator.contains("Выйти из группы"));
    assert!(!previous_creator.contains(&format!("action=\"{path}/transfer\"")));
    let new_creator = text(call(app, "GET", path, &cookies[2], "", &[]).await).await;
    assert!(new_creator.contains(&format!("action=\"{path}/transfer\"")));
    assert!(!new_creator.contains(&format!("action=\"{path}/leave\"")));
    assert_eq!(
        call(
            app,
            "POST",
            &format!("{path}/leave"),
            &cookies[2],
            &format!("csrf_token={}", tokens[2]),
            &[],
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            app,
            "POST",
            &transfer_path,
            &cookies[0],
            &transfer_to(users[2], &tokens[0]),
            &[],
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            app,
            "POST",
            &format!("{path}/leave"),
            &cookies[0],
            &format!("csrf_token={}", tokens[0]),
            &[],
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(app, "GET", path, &cookies[0], "", &[]).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(app, "GET", path, &cookies[2], "", &[]).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM social_group_members WHERE group_id={group_id} AND role='creator' AND status='active'"
            ),
        )
        .await,
        "1"
    );
}
