//! S030 verified Book owner service: A's step-up token and recipient tier are
//! checked against authoritative locked PostgreSQL facts; every grant and
//! ownership change is serialized behind the same Book row.
use super::{swap_owner_in_verified_transaction, TrustedTransferPermit};
use crate::repositories::{
    book_step_up::{consume_book_step_up, BookStepUpScope},
    owner_transfer_tier::verified_book_tier,
};
use crate::services::authoring::AuthorError;
use crate::services::owner_transfer_policy::{
    authorize_owner_transfer, AccessTier, BookTransferState, LayerTransferState,
    OwnerTransferDenial, OwnerTransferFacts, RevenueTransferState,
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

const OWNER_TRANSFER_SCOPE: &str = "book.owner.transfer.v1";

pub fn owner_transfer_payload_digest(
    actor: i64,
    book_id: i64,
    expected_owner: i64,
    target_display_name: &str,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(OWNER_TRANSFER_SCOPE.as_bytes());
    digest.update(actor.to_be_bytes());
    digest.update(book_id.to_be_bytes());
    digest.update(expected_owner.to_be_bytes());
    digest.update((target_display_name.len() as u64).to_be_bytes());
    digest.update(target_display_name.as_bytes());
    digest.finalize().into()
}
/// S030: a Book owner operation with A/B/D/B-10 proofs in one PostgreSQL
/// transaction. The handler supplies an A-issued one-time step-up token,
/// never an HTTP-forgeable Book permit.
#[derive(Clone)]
pub struct PgOwnerTransfer {
    pool: PgPool,
}

#[derive(Clone, Copy)]
pub struct OwnerTransferPrecondition {
    pub owner_id: i64,
    pub revision: i64,
}

pub struct BookOwnershipSettings {
    pub title: String,
    pub access_level: Option<String>,
    pub owner_id: i64,
    pub ownership_revision: i64,
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
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let book = sqlx::query(
            "SELECT b.title,b.access_level,b.owner_user_id,s.current_revision,
                    COALESCE((SELECT MAX(e.id)+1 FROM book_owner_transfer_events e
                              WHERE e.book_id=b.id),1) AS ownership_revision
             FROM books b
             LEFT JOIN revenue_share_states s ON s.book_id=b.id
             WHERE b.id=$1 AND b.owner_user_id=$2
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND b.lifecycle_status NOT IN ('pending_deletion','ordinary_deleted')
               AND EXISTS(SELECT 1 FROM book_members m
                          WHERE m.book_id=b.id AND m.user_id=$2
                            AND m.is_active AND m.role='owner')
             FOR SHARE OF b",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some(book) = book else {
            return Ok(None);
        };
        let revision: Option<i64> = book
            .try_get("current_revision")
            .map_err(|_| AuthorError::Unavailable)?;
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
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if rows.len() > 100 {
            return Err(AuthorError::Unavailable);
        }
        let contributors = rows
            .into_iter()
            .map(|row| {
                Ok(BookContributor {
                    user_id: row
                        .try_get("user_id")
                        .map_err(|_| AuthorError::Unavailable)?,
                    display_name: row
                        .try_get("display_name")
                        .map_err(|_| AuthorError::Unavailable)?,
                    role: row.try_get("role").map_err(|_| AuthorError::Unavailable)?,
                    share_percent: row
                        .try_get("current_share")
                        .map_err(|_| AuthorError::Unavailable)?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(Some(BookOwnershipSettings {
            title: book
                .try_get("title")
                .map_err(|_| AuthorError::Unavailable)?,
            access_level: book
                .try_get("access_level")
                .map_err(|_| AuthorError::Unavailable)?,
            owner_id: book
                .try_get("owner_user_id")
                .map_err(|_| AuthorError::Unavailable)?,
            ownership_revision: book
                .try_get("ownership_revision")
                .map_err(|_| AuthorError::Unavailable)?,
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
        expected: OwnerTransferPrecondition,
        target_display_name: &str,
        session_digest: &[u8; 32],
        step_up_token: &str,
    ) -> Result<(), AuthorError> {
        let expected_owner = expected.owner_id;
        let expected_ownership_revision = expected.revision;
        if actor <= 0
            || book_id <= 0
            || expected_owner != actor
            || expected_ownership_revision <= 0
            || target_display_name.is_empty()
            || target_display_name.trim() != target_display_name
            || target_display_name.chars().count() > 80
            || step_up_token.is_empty()
        {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        // Consume exactly one A-issued, session-bound confirmation for this
        // Book, current owner and precise target nickname, with row locks.
        let payload =
            owner_transfer_payload_digest(actor, book_id, expected_owner, target_display_name);
        if !consume_book_step_up(
            &mut tx,
            actor,
            session_digest,
            step_up_token,
            book_id,
            expected_ownership_revision,
            BookStepUpScope {
                name: OWNER_TRANSFER_SCOPE,
                payload_digest: &payload,
            },
        )
        .await
        .map_err(|_| AuthorError::Unavailable)?
        {
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
            return Err(if recipients.is_empty() {
                AuthorError::Missing
            } else {
                AuthorError::Conflict
            });
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
            "SELECT owner_user_id, NOT admin_hold AND NOT is_deleted
               AND lifecycle_status NOT IN ('pending_deletion','ordinary_deleted') AS available
             FROM books WHERE id=$1 FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Missing)?;
        let actual_owner: Option<i64> = row
            .try_get("owner_user_id")
            .map_err(|_| AuthorError::Unavailable)?;
        let available: bool = row
            .try_get("available")
            .map_err(|_| AuthorError::Unavailable)?;
        // An unconsumed confirmation must not survive an A -> B -> A cycle.
        // The append-only transfer event ID provides a monotonic Book epoch.
        let live_ownership_revision: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(id)+1,1) FROM book_owner_transfer_events
             WHERE book_id=$1",
        )
        .bind(book_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if live_ownership_revision != expected_ownership_revision {
            return Err(AuthorError::Conflict);
        }
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
        // A-owned facts are read through the canonical persisted tiers.
        // Lock active recipient account/grant rows until the swap commits.
        let adult: Option<bool> = sqlx::query_scalar(
            "SELECT adult_enabled FROM users
             WHERE id=$1 AND account_state='active' FOR SHARE",
        )
        .bind(recipient)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let recipient_tier = if let Some(adult) = adult {
            let pro: Option<i64> = sqlx::query_scalar(
                "SELECT user_id FROM user_level_grants
                 WHERE user_id=$1 AND level='pro_adult' AND is_active FOR SHARE",
            )
            .bind(recipient)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            Some(if pro.is_some() {
                AccessTier::ProAdult
            } else if adult {
                AccessTier::Adult
            } else {
                AccessTier::Basic
            })
        } else {
            None
        };

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
            book_state: if available {
                BookTransferState::Available
            } else {
                BookTransferState::HeldOrDeleted
            },
            book_tier,
            recipient_effective_tier: recipient_tier,
            layer_state: if ever_in_layer {
                LayerTransferState::Unverified
            } else {
                LayerTransferState::VerifiedClearIncludingHistorical
            },
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
            &mut tx,
            actor,
            book_id,
            expected_owner,
            recipient,
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
