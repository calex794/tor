//! B-09c / S030: atomic owner-row swap under one PostgreSQL Book lock.
//!
//! This is deliberately a *sealed*, crate-internal persistence primitive.
//! Before it may be called from a production B service, that service must
//! prove recipient effective tier, Book tier, active Layer rights (including
//! those inherited from historical Layer compositions), financial snapshots,
//! fresh session/step-up and the exact Book revision in the SAME boundary.
//! No HTTP or public export is allowed to construct the permit yet.
use crate::services::authoring::AuthorError;
use sqlx::{PgPool, Postgres, Row, Transaction};
use tor_accounts::{
    domain::access_level::EffectiveUserTier,
    repositories::access_levels::verified_effective_user_tier,
    services::security::VerifiedPassword,
};
use crate::repositories::owner_transfer_tier::verified_book_tier;
use crate::services::owner_transfer_policy::{
    authorize_owner_transfer, AccessTier, BookTransferState, LayerTransferState,
    OwnerTransferDenial, OwnerTransferFacts, RevenueTransferState,
};

/// No production constructor: only the test below can mint this capability.
/// Do not expose it until the account/Layer/revenue verification ports exist.
pub(crate) struct TrustedTransferPermit {
    _issuer: (),
}

/// All A/B/D/B-10 checks and their row locks must be performed by the
/// caller in this SAME transaction before it can construct a trusted permit.
/// This primitive only updates owner rows; it is never an HTTP permission.
#[allow(dead_code)]
pub(crate) async fn swap_owner_in_verified_transaction(
    tx: &mut Transaction<'_, Postgres>,
    actor_id: i64,
    book_id: i64,
    expected_owner_id: i64,
    recipient_id: i64,
    _permit: TrustedTransferPermit,
) -> Result<(), AuthorError> {
    if actor_id <= 0 || book_id <= 0 || recipient_id <= 0 || expected_owner_id <= 0 {
        return Err(AuthorError::Invalid);
    }
    if actor_id != expected_owner_id || expected_owner_id == recipient_id {
        return Err(AuthorError::Forbidden);
    }
    let current: Option<i64> = sqlx::query_scalar(
        "SELECT owner_user_id FROM books
         WHERE id=$1 AND NOT admin_hold AND NOT is_deleted
         FOR UPDATE",
    )
    .bind(book_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if current != Some(expected_owner_id) {
        return Err(AuthorError::Conflict);
    }

    let active_owner: Option<i64> = sqlx::query_scalar(
        "SELECT user_id FROM book_members
         WHERE book_id=$1 AND is_active AND role='owner' FOR UPDATE",
    )
    .bind(book_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if active_owner != Some(expected_owner_id) {
        return Err(AuthorError::Conflict);
    }

    // Demote first so the partial unique OWNER index cannot observe two
    // owners, even briefly within this transaction.
    let demoted: Option<i64> = sqlx::query_scalar(
        "UPDATE book_members SET role='translator',updated_at=now()
         WHERE book_id=$1 AND user_id=$2 AND is_active AND role='owner'
         RETURNING user_id",
    )
    .bind(book_id)
    .bind(expected_owner_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if demoted != Some(expected_owner_id) {
        return Err(AuthorError::Conflict);
    }

    let promoted: Option<i64> = sqlx::query_scalar(
        "INSERT INTO book_members(book_id,user_id,role,is_active,share_percent)
         SELECT $1,id,'owner',TRUE,0 FROM users WHERE id=$2
         ON CONFLICT(book_id,user_id) DO UPDATE
         SET role='owner',is_active=TRUE,updated_at=now()
         RETURNING user_id",
    )
    .bind(book_id)
    .bind(recipient_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if promoted != Some(recipient_id) {
        return Err(AuthorError::Missing);
    }

    let cas: Option<i64> = sqlx::query_scalar(
        "UPDATE books SET owner_user_id=$3
         WHERE id=$1 AND owner_user_id=$2
           AND NOT admin_hold AND NOT is_deleted
         RETURNING id",
    )
    .bind(book_id)
    .bind(expected_owner_id)
    .bind(recipient_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if cas != Some(book_id) {
        return Err(AuthorError::Conflict);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{PgPool, Row};

    async fn pool() -> PgPool {
        let url = std::env::var("TEST_DATABASE_URL").expect("PG CI URL");
        assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
        PgPool::connect(&url)
            .await
            .expect("isolated test PostgreSQL")
    }

    async fn user(pool: &PgPool, id: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'B-09 CAS fixture','$argon2id$test')
             ON CONFLICT(email_normalized) DO UPDATE
             SET display_name=EXCLUDED.display_name RETURNING id",
        )
        .bind(format!("b09c-{id}@tor-ci.example"))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn fixture(pool: &PgPool, label: &str) -> (i64, i64, i64, i64) {
        let from = user(pool, &format!("{label}-from")).await;
        let to_a = user(pool, &format!("{label}-to-a")).await;
        let to_b = user(pool, &format!("{label}-to-b")).await;
        let book_id = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES($1,'ru','B09 atomic owner swap','draft',$2) RETURNING id",
        )
        .bind(format!("ci:b09c:{label}:owner:{from}"))
        .bind(from)
        .fetch_one(pool)
        .await
        .unwrap();
        (book_id, from, to_a, to_b)
    }

    fn permit() -> TrustedTransferPermit {
        TrustedTransferPermit { _issuer: () }
    }

    /// Test-only stand-in for the future B-owned service: one database
    /// transaction must encompass evidence validation, owner swap and commit.
    async fn fixture_transfer(
        pool: &PgPool,
        actor: i64,
        book_id: i64,
        expected_owner: i64,
        recipient: i64,
    ) -> Result<(), AuthorError> {
        let mut tx = pool.begin().await.map_err(|_| AuthorError::Unavailable)?;
        let result = swap_owner_in_verified_transaction(
            &mut tx,
            actor,
            book_id,
            expected_owner,
            recipient,
            permit(),
        )
        .await;
        match result {
            Ok(()) => tx.commit().await.map_err(|_| AuthorError::Unavailable),
            Err(error) => {
                tx.rollback().await.map_err(|_| AuthorError::Unavailable)?;
                Err(error)
            }
        }
    }

    async fn owner_and_count(pool: &PgPool, book_id: i64) -> (i64, i64) {
        let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
            .bind(book_id)
            .fetch_one(pool)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM book_members WHERE book_id=$1
             AND role='owner' AND is_active",
        )
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap();
        (owner, count)
    }

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn b_09_pg_owner_swap_preserves_membership_shares_and_one_owner() {
        let pool = pool().await;
        let (book_id, from, to, _) = fixture(&pool, "success").await;
        sqlx::query(
            "UPDATE book_members SET share_percent=21
             WHERE book_id=$1 AND user_id=$2",
        )
        .bind(book_id)
        .bind(from)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO book_members(book_id,user_id,role,share_percent)
             VALUES($1,$2,'translator',13)",
        )
        .bind(book_id)
        .bind(to)
        .execute(&pool)
        .await
        .unwrap();
        fixture_transfer(&pool, from, book_id, from, to)
            .await
            .unwrap();
        assert_eq!(owner_and_count(&pool, book_id).await, (to, 1));
        let rows = sqlx::query(
            "SELECT user_id,role,is_active,share_percent FROM book_members
             WHERE book_id=$1 ORDER BY user_id",
        )
        .bind(book_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            let id: i64 = row.get("user_id");
            let role: String = row.get("role");
            let share: i16 = row.get("share_percent");
            assert!(row.get::<bool, _>("is_active"));
            if id == from {
                assert_eq!(role, "translator");
                assert_eq!(share, 21);
            } else {
                assert_eq!(id, to);
                assert_eq!(role, "owner");
                assert_eq!(share, 13);
            }
        }
        assert_eq!(
            fixture_transfer(&pool, from, book_id, from, to).await,
            Err(AuthorError::Conflict),
            "former owner cannot replay the transfer"
        );
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn b_09_pg_owner_swap_missing_recipient_rolls_back_demote() {
        let pool = pool().await;
        let (book_id, from, _, _) = fixture(&pool, "rollback").await;
        assert_eq!(
            fixture_transfer(&pool, from, book_id, from, i64::MAX - 5).await,
            Err(AuthorError::Missing)
        );
        assert_eq!(owner_and_count(&pool, book_id).await, (from, 1));
        let role: String =
            sqlx::query_scalar("SELECT role FROM book_members WHERE book_id=$1 AND user_id=$2")
                .bind(book_id)
                .bind(from)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(role, "owner");
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn b_09_pg_owner_swap_racing_cas_has_one_winner() {
        let pool = pool().await;
        let (book_id, from, to_a, to_b) = fixture(&pool, "race").await;
        let (a, b) = tokio::join!(
            fixture_transfer(&pool, from, book_id, from, to_a),
            fixture_transfer(&pool, from, book_id, from, to_b)
        );
        assert_eq!([a, b].iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            [a, b]
                .iter()
                .filter(|r| **r == Err(AuthorError::Conflict))
                .count(),
            1
        );
        let (actual_owner, count) = owner_and_count(&pool, book_id).await;
        assert!(actual_owner == to_a || actual_owner == to_b);
        assert_eq!(count, 1);
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn b_09_pg_owner_swap_denies_stale_actor_and_held_book() {
        let pool = pool().await;
        let (book_id, from, to, outsider) = fixture(&pool, "deny").await;
        assert_eq!(
            fixture_transfer(&pool, outsider, book_id, from, to).await,
            Err(AuthorError::Forbidden)
        );
        sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
            .bind(book_id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            fixture_transfer(&pool, from, book_id, from, to).await,
            Err(AuthorError::Conflict)
        );
        assert_eq!(owner_and_count(&pool, book_id).await, (from, 1));
        pool.close().await;
    }
}


/// S030: a Book owner operation with A/B/D/B-10 proofs in one PostgreSQL
/// transaction. An HTTP handler can supply only a freshly verified A password
/// capability; it cannot forge the sealed Book mutation permit.
#[derive(Clone)]
pub struct PgOwnerTransfer {
    pool: PgPool,
}

pub struct BookOwnershipSettings {
    pub title: String,
    pub access_level: Option<String>,
    pub owner_id: i64,
    pub share_revision: Option<i64>,
    pub contributors: Vec<BookContributor>,
}

pub struct BookContributor {
    pub user_id: i64,
    pub display_name: String,
    pub role: String,
    pub share_percent: i16,
}

impl PgOwnerTransfer {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A private snapshot for the current canonical owner only. A Team owner
    /// and a former Book owner never inherit this financial management view.
    pub async fn settings(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<BookOwnershipSettings>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let book = sqlx::query(
            "SELECT b.title,b.access_level,b.owner_user_id,s.current_revision
             FROM books b
             LEFT JOIN revenue_share_states s ON s.book_id=b.id
             WHERE b.id=$1 AND b.owner_user_id=$2
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND EXISTS(SELECT 1 FROM book_members m
                          WHERE m.book_id=b.id AND m.user_id=$2
                            AND m.is_active AND m.role='owner')",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some(book) = book else {
            return Ok(None);
        };
        let revision: Option<i64> = book.try_get("current_revision").map_err(|_| AuthorError::Unavailable)?;
        let rows = sqlx::query(
            "SELECT m.user_id,u.display_name,m.role,
                    COALESCE(a.share_percent,0)::SMALLINT AS current_share
             FROM book_members m
             JOIN users u ON u.id=m.user_id
             LEFT JOIN revenue_share_assignments a
               ON a.book_id=m.book_id AND a.revision=$2 AND a.user_id=m.user_id
             WHERE m.book_id=$1 AND m.is_active
             ORDER BY m.user_id LIMIT 101",
        )
        .bind(book_id)
        .bind(revision)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if rows.len() > 100 {
            return Err(AuthorError::Unavailable);
        }
        let contributors = rows
            .into_iter()
            .map(|row| {
                Ok(BookContributor {
                    user_id: row.try_get("user_id").map_err(|_| AuthorError::Unavailable)?,
                    display_name: row.try_get("display_name").map_err(|_| AuthorError::Unavailable)?,
                    role: row.try_get("role").map_err(|_| AuthorError::Unavailable)?,
                    share_percent: row.try_get("current_share").map_err(|_| AuthorError::Unavailable)?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(BookOwnershipSettings {
            title: book.try_get("title").map_err(|_| AuthorError::Unavailable)?,
            access_level: book.try_get("access_level").map_err(|_| AuthorError::Unavailable)?,
            owner_id: book.try_get("owner_user_id").map_err(|_| AuthorError::Unavailable)?,
            share_revision: revision,
            contributors,
        }))
    }

    /// Transfer by exact display name, as in NEwBOO. Ambiguous nicknames are
    /// rejected rather than arbitrarily choosing another user's account.
    /// Existing Team links are retained only when the recipient is an active
    /// member/owner of each linked Team. All Layer history is currently a
    /// conservative hard stop: even an expired term can be renewed into an
    /// old composition, so "no live terms" alone is not a safe clearance.
    pub async fn transfer(
        &self,
        actor: i64,
        book_id: i64,
        expected_owner: i64,
        target_display_name: &str,
        session_digest: &[u8; 32],
        proof: &VerifiedPassword,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || book_id <= 0 || expected_owner != actor
            || target_display_name.is_empty()
            || target_display_name.trim() != target_display_name
            || target_display_name.chars().count() > 80
            || !proof.is_fresh()
        {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| AuthorError::Unavailable)?;
        // Lock A in the same order as password rotation: user before session.
        // Re-read both under their locks after Argon2 has completed.
        let stored_hash: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users
             WHERE id=$1 AND account_state='active' FOR SHARE",
        )
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if !stored_hash.as_deref().is_some_and(|hash| proof.matches_hash(hash)) {
            return Err(AuthorError::Forbidden);
        }
        let live: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM sessions WHERE user_id=$1 AND token_hash=$2
             AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE",
        )
        .bind(actor)
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if live != Some(actor) || !proof.is_fresh() {
            return Err(AuthorError::Forbidden);
        }

        let recipients: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE display_name=$1 AND account_state='active'
             ORDER BY id LIMIT 2 FOR SHARE",
        )
        .bind(target_display_name)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if recipients.len() != 1 {
            return Err(if recipients.is_empty() { AuthorError::Missing } else { AuthorError::Conflict });
        }
        let recipient = recipients[0];

        // S028 acquires Team before Book. Use that order as well, so a
        // concurrent attach/detach/member removal cannot deadlock with a
        // transfer or silently strand its existing association.
        let team_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT t.id FROM translation_teams t
             JOIN translation_team_books tb ON tb.team_id=t.id
             WHERE tb.book_id=$1 AND tb.is_active
             ORDER BY t.id FOR SHARE OF t",
        )
        .bind(book_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;

        let row = sqlx::query(
            "SELECT owner_user_id, NOT admin_hold AND NOT is_deleted AS available
             FROM books WHERE id=$1 FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Missing)?;
        let actual_owner: Option<i64> = row.try_get("owner_user_id").map_err(|_| AuthorError::Unavailable)?;
        let available: bool = row.try_get("available").map_err(|_| AuthorError::Unavailable)?;
        let current_teams: Vec<i64> = sqlx::query_scalar(
            "SELECT team_id FROM translation_team_books
             WHERE book_id=$1 AND is_active ORDER BY team_id",
        )
        .bind(book_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if team_ids != current_teams {
            // A Team was attached after our initial lock scan; retry instead
            // of approving a team whose membership was not locked.
            return Err(AuthorError::Conflict);
        }
        for team in &team_ids {
            let eligible: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                   SELECT 1 FROM translation_teams t
                   JOIN users u ON u.id=$2 AND u.account_state='active'
                   WHERE t.id=$1 AND
                     (t.owner_user_id=$2 OR EXISTS(
                       SELECT 1 FROM translation_team_members m
                       WHERE m.team_id=t.id AND m.user_id=$2))
                 )",
            )
            .bind(team)
            .bind(recipient)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            if !eligible {
                return Err(AuthorError::Forbidden);
            }
        }

        let active_owner_member_id: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM book_members
             WHERE book_id=$1 AND user_id=$2 AND is_active AND role='owner'
             FOR SHARE",
        )
        .bind(book_id)
        .bind(expected_owner)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let book_tier = verified_book_tier(&mut tx, book_id).await?;
        let recipient_tier = verified_effective_user_tier(&mut tx, recipient)
            .await
            .map_err(|_| AuthorError::Unavailable)?
            .map(|tier| match tier {
                EffectiveUserTier::Basic => AccessTier::Basic,
                EffectiveUserTier::Adult => AccessTier::Adult,
                EffectiveUserTier::ProAdult => AccessTier::ProAdult,
            });

        // The complete Layer history matters. New Layer purchases lock every
        // historical Book FOR SHARE, whereas composition mutations lock Book
        // FOR UPDATE. Our Book lock prevents both classes of race.
        let ever_in_layer: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce_layer_version_books
             WHERE book_id=$1)",
        )
        .bind(book_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let revenue_revision: Option<i64> = sqlx::query_scalar(
            "SELECT current_revision FROM revenue_share_states
             WHERE book_id=$1 FOR SHARE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let missing_purchase_snapshot: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM chapter_purchase_facts p
               JOIN chapters c ON c.id=p.chapter_id
               LEFT JOIN chapter_purchase_share_snapshots s ON s.purchase_id=p.id
               WHERE c.book_id=$1 AND (s.purchase_id IS NULL OR s.book_id<>$1)
             )",
        )
        .bind(book_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let facts = OwnerTransferFacts {
            actor_id: actor,
            book_id,
            expected_owner_id: expected_owner,
            actual_owner_id: actual_owner.unwrap_or(0),
            recipient_id: recipient,
            active_owner_member_id,
            book_state: if available { BookTransferState::Available }
                else { BookTransferState::HeldOrDeleted },
            book_tier,
            recipient_effective_tier: recipient_tier,
            layer_state: if ever_in_layer { LayerTransferState::Unverified }
                else { LayerTransferState::VerifiedClearIncludingHistorical },
            revenue_state: if revenue_revision.is_some() && !missing_purchase_snapshot {
                RevenueTransferState::HistoricalPurchasesPreserved
            } else {
                RevenueTransferState::Unverified
            },
        };
        authorize_owner_transfer(facts).map_err(|denial| match denial {
            OwnerTransferDenial::StaleOrUnverifiedOwner
            | OwnerTransferDenial::UnverifiedLayerEntitlements
            | OwnerTransferDenial::ExistingLayerEntitlement
            | OwnerTransferDenial::UnverifiedRevenueSnapshots
            | OwnerTransferDenial::RevenueSnapshotsWouldChange => AuthorError::Conflict,
            _ => AuthorError::Forbidden,
        })?;
        // The permit can only be instantiated here after server-owned facts
        // have been checked under the same Book + A + Team row locks.
        swap_owner_in_verified_transaction(
            &mut tx, actor, book_id, expected_owner, recipient,
            TrustedTransferPermit { _issuer: () },
        )
        .await?;
        sqlx::query(
            "INSERT INTO book_owner_transfer_events
             (book_id,previous_owner_user_id,new_owner_user_id,share_revision)
             VALUES($1,$2,$3,$4)",
        )
        .bind(book_id)
        .bind(expected_owner)
        .bind(recipient)
        .bind(revenue_revision.ok_or(AuthorError::Conflict)?)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)
    }
}


#[cfg(test)]
mod verified_transfer_tests {
    use super::*;
    use crate::domain::shares::ShareAssignment;
    use crate::repositories::{
        revenue_shares::PgRevenueShares, translation_team_books::PgTeamBooks,
    };
    use tor_accounts::services::security::{
        hash_password, session_token_digest, verify_password_for_step_up,
    };

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
            "INSERT INTO books(source_identity,target_language,title,
                               visibility,owner_user_id,access_level)
             VALUES($1,'ru','S030 transfer fixture','draft',$2,$3)
             RETURNING id",
        )
        .bind(format!("ci:s030:{key}:{owner}"))
        .bind(owner)
        .bind(level)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn session(pool: &PgPool, actor: i64, label: &str) -> [u8; 32] {
        let digest = session_token_digest(label);
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

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn s030_pg_transfer_preserves_team_share_revisions_and_one_owner() {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        assert!(url.contains("127.0.0.1:5432") && url.ends_with("/tor_ci"));
        let pool = PgPool::connect(&url).await.unwrap();
        let password = "S030OwnerTransferStrongPassword";
        let hash = hash_password(password).unwrap();
        let from = user(&pool, "good-owner", "S030 good owner", &hash).await;
        let to = user(&pool, "good-recipient", "S030 good recipient", &hash).await;
        let digest = session(&pool, from, "s030-good-owner-session").await;
        let to_digest = session(&pool, to, "s030-good-recipient-session").await;
        let book_id = book(&pool, "good", from, "basic").await;
        let team: i64 = sqlx::query_scalar(
            "INSERT INTO translation_teams(owner_user_id,name)
             VALUES($1,'S030 transfer continuity') RETURNING id",
        )
        .bind(from)
        .fetch_one(&pool)
        .await
        .unwrap();
        let invite = session_token_digest("s030-team-invite-accepted");
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
        let repo = PgOwnerTransfer::new(pool.clone());
        let proof = verify_password_for_step_up(password, &hash).unwrap();
        repo.transfer(from, book_id, from, "S030 good recipient", &digest, &proof)
            .await
            .unwrap();
        let current_owner: i64 = sqlx::query_scalar(
            "SELECT owner_user_id FROM books WHERE id=$1",
        )
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(current_owner, to);
        let active_owners: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM book_members
             WHERE book_id=$1 AND role='owner' AND is_active",
        )
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active_owners, 1);
        let recorded: i64 = sqlx::query_scalar(
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
        assert_eq!(recorded, 1);
        let team_view = PgTeamBooks::new(pool.clone()).view(to, team).await.unwrap();
        assert!(team_view.linked.iter().any(|v| v.id == book_id));
        assert!(repo.settings(from, book_id).await.unwrap().is_none());
        assert!(repo.settings(to, book_id).await.unwrap().is_some());
        let duplicate = verify_password_for_step_up(password, &hash).unwrap();
        assert_eq!(
            repo.transfer(from, book_id, from, "S030 good recipient", &digest, &duplicate).await,
            Err(AuthorError::Conflict),
        );
        let shares = PgRevenueShares::new(pool.clone());
        let to_proof = verify_password_for_step_up(password, &hash).unwrap();
        let next = shares.replace_schedule_verified(
            to, book_id, 1,
            &[
                ShareAssignment { user_id: from, share_percent: 65 },
                ShareAssignment { user_id: to, share_percent: 35 },
            ],
            "new owner revision", &to_digest, &to_proof,
        ).await.unwrap();
        assert_eq!(next, 2);
        let previous: i16 = sqlx::query_scalar(
            "SELECT share_percent FROM revenue_share_assignments
             WHERE book_id=$1 AND revision=1 AND user_id=$2",
        )
        .bind(book_id)
        .bind(from)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(previous, 100, "historic owner share must not be rewritten");
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn s030_pg_denies_low_tier_historical_layer_and_revoked_session() {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        assert!(url.contains("127.0.0.1:5432") && url.ends_with("/tor_ci"));
        let pool = PgPool::connect(&url).await.unwrap();
        let password = "S030OwnerTransferDeniedPassword";
        let hash = hash_password(password).unwrap();
        let from = user(&pool, "deny-owner", "S030 deny owner", &hash).await;
        let to = user(&pool, "deny-recipient", "S030 deny recipient", &hash).await;
        let digest = session(&pool, from, "s030-denied-owner-session").await;
        let book_id = book(&pool, "deny-adult", from, "adult").await;
        let repo = PgOwnerTransfer::new(pool.clone());
        let low_tier = verify_password_for_step_up(password, &hash).unwrap();
        assert_eq!(
            repo.transfer(from, book_id, from, "S030 deny recipient", &digest, &low_tier).await,
            Err(AuthorError::Forbidden),
        );
        sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
            .bind(to)
            .execute(&pool)
            .await
            .unwrap();
        let layer: i64 = sqlx::query_scalar(
            "INSERT INTO commerce_layers(owner_user_id,title)
             VALUES($1,'S030 historical layer') RETURNING id",
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
        let layer_block = verify_password_for_step_up(password, &hash).unwrap();
        assert_eq!(
            repo.transfer(from, book_id, from, "S030 deny recipient", &digest, &layer_block).await,
            Err(AuthorError::Conflict),
        );
        sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
            .bind(digest.as_slice())
            .execute(&pool)
            .await
            .unwrap();
        let revoked = verify_password_for_step_up(password, &hash).unwrap();
        assert_eq!(
            repo.transfer(from, book_id, from, "S030 deny recipient", &digest, &revoked).await,
            Err(AuthorError::Forbidden),
        );
        let current_owner: i64 = sqlx::query_scalar(
            "SELECT owner_user_id FROM books WHERE id=$1",
        )
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(current_owner, from);
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM book_owner_transfer_events WHERE book_id=$1",
        )
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(events, 0);
        pool.close().await;
    }
}
