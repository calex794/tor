//! S020/#486: NEwBOO site administrator's reader bypass is independent of
//! Book moderation state, lifecycle and the ordinary subscriber's age tier.
//! Run from S065's real disposable-PG/Axum role matrix. No G signing here.
use super::super::{pg, read_page};
use axum::{http::StatusCode, Router};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::session_token_digest,
};
use tor_commerce::repositories::admin_reader_entitlements::PgAdministratorReaderEntitlements;
use tor_contracts::paid_delivery::{
    Actor, EntitlementDecision, EntitlementPort, PositiveId,
};

struct RestoreBook<'a> {
    url: &'a str,
    book_id: i64,
}
impl Drop for RestoreBook<'_> {
    fn drop(&mut self) {
        // Never leak a test-only blocked/unlisted state into subsequent
        // PostgreSQL scenarios, including after a failed assertion.
        pg(
            self.url,
            &format!(
                "UPDATE books SET access_level='pro_adult',visibility_status='active',\
                 lifecycle_status='in_progress' WHERE id={}",
                self.book_id
            ),
        );
    }
}

pub(super) async fn assert_admin_can_inspect_all_book_states(
    url: &str,
    accounts: &PgAccounts,
    reader: &Router,
    admin_id: i64,
    admin_session: &str,
    book: i64,
    chapter: i64,
    version: i64,
) {
    let _restore = RestoreBook { url, book_id: book };
    let authority = PgAdministratorReaderEntitlements::new(
        accounts.pool().clone(),
        session_token_digest(admin_session),
    );
    let admin = Actor {
        user_id: PositiveId::new(admin_id).unwrap(),
    };
    let chapter_id = PositiveId::new(chapter).unwrap();
    let path = format!("/read/admin/{chapter}");

    // A non-indexable Book may remain accessible to a live site
    // administrator, without granting the same access to a moderator,
    // buyer, crawler or ordinary 18+/21+ viewer.
    for visibility in ["pending_moderation", "hidden", "blocked"] {
        pg(
            url,
            &format!(
                "UPDATE books SET access_level='pro_adult',\
                 visibility_status='{visibility}' WHERE id={book}"
            ),
        );
        let decision = authority
            .resolve_entitlement(admin, chapter_id, None)
            .await
            .unwrap();
        assert!(
            matches!(
                decision,
                EntitlementDecision::Allowed { pinned }
                    if pinned.chapter_id.get() == chapter
                        && pinned.version_id.get() == version
            ),
            "live site administrator must retain CURRENT sealed entitlement at {visibility}"
        );
        assert_eq!(
            read_page(reader, &path, Some(admin_session)).await.status(),
            StatusCode::OK,
            "private admin shell must survive Book moderation status {visibility}"
        );
    }

    // pending_deletion unlists the Book; it is not a hard deletion and does
    // not silently revoke the site administrator's separate inspection role.
    pg(
        url,
        &format!(
            "UPDATE books SET visibility_status='active',\
             lifecycle_status='pending_deletion' WHERE id={book}"
        ),
    );
    let decision = authority
        .resolve_entitlement(admin, chapter_id, None)
        .await
        .unwrap();
    assert!(
        matches!(
            decision,
            EntitlementDecision::Allowed { pinned }
                if pinned.version_id.get() == version
        ),
        "pending_deletion cannot revoke the site administrator's CURRENT edition"
    );
    assert_eq!(
        read_page(reader, &path, Some(admin_session)).await.status(),
        StatusCode::OK
    );
}
