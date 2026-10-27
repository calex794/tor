//! Durable private ranking. SQLx holds one user lock through every mutation.
//! Do not expose StartVerified over HTTP before C-owned completion exists.
use crate::services::ranking::{
    AnswerStatus, BookRank, Outcome, PlacementState, RankingError, RankingSnapshot,
    MAX_RANKED_BOOKS,
};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct PgRanking {
    pool: PgPool,
}

#[derive(Clone, Debug)]
pub enum RankingCommand {
    /// The caller must have checked a trusted chapter-completion record.
    StartVerified(i64),
    Answer {
        expected_state_version: u64,
        comparison_id: String,
        outcome: Outcome,
    },
    ResumeExhausted,
}

#[derive(Clone, Debug)]
pub struct RankingCommit {
    pub snapshot: RankingSnapshot,
    pub status: Option<AnswerStatus>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RankingStoreError {
    Invalid,
    Conflict,
    Stale,
    Exhausted,
    Unavailable,
}

impl From<RankingError> for RankingStoreError {
    fn from(value: RankingError) -> Self {
        match value {
            RankingError::Invalid => Self::Invalid,
            RankingError::Conflict => Self::Conflict,
            RankingError::Stale => Self::Stale,
            RankingError::Exhausted => Self::Exhausted,
        }
    }
}
impl From<sqlx::Error> for RankingStoreError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

#[derive(Clone, Debug)]
struct AnswerEvidence {
    epoch: i64,
    version: i64,
    comparison_id: String,
    a: i64,
    b: i64,
    outcome: &'static str,
}

fn outcome_name(value: Outcome) -> &'static str {
    match value {
        Outcome::AWins => "a_wins",
        Outcome::BWins => "b_wins",
        Outcome::Tie => "tie",
        Outcome::Skip => "skip",
        Outcome::DeferA => "defer_a",
        Outcome::DeferB => "defer_b",
    }
}

fn db_u64(value: u64) -> Result<i64, RankingStoreError> {
    i64::try_from(value).map_err(|_| RankingStoreError::Invalid)
}

impl PgRanking {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Private snapshot. The caller must derive user_id from the session.
    pub async fn snapshot(&self, user_id: i64) -> Result<RankingSnapshot, RankingStoreError> {
        if user_id <= 0 {
            return Err(RankingStoreError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let revision: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM user_book_ranking_head WHERE user_id=$1 FOR SHARE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let result = load_snapshot(&mut tx, user_id, revision.unwrap_or(0)).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Explicit expected_revision is a second fence around state_version:
    /// two devices cannot both commit even if the answer is a skip.
    pub async fn apply(
        &self,
        user_id: i64,
        expected_revision: u64,
        command: RankingCommand,
    ) -> Result<RankingCommit, RankingStoreError> {
        if user_id <= 0 {
            return Err(RankingStoreError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO user_book_ranking_head(user_id,revision)
             SELECT id,0 FROM users WHERE id=$1
             ON CONFLICT (user_id) DO NOTHING",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        let revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM user_book_ranking_head WHERE user_id=$1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RankingStoreError::Invalid)?;
        let before = load_snapshot(&mut tx, user_id, revision).await?;
        if before.revision != expected_revision {
            return Err(RankingStoreError::Stale);
        }

        let evidence = if let RankingCommand::Answer { outcome, .. } = &command {
            let state = before.placement.as_ref().ok_or(RankingStoreError::Stale)?;
            let question = before
                .next_question()?
                .ok_or(RankingStoreError::Exhausted)?;
            Some(AnswerEvidence {
                epoch: db_u64(state.placement_epoch)?,
                version: db_u64(state.state_version)?,
                comparison_id: question.comparison_id,
                a: question.book_a_id,
                b: question.book_b_id,
                outcome: outcome_name(*outcome),
            })
        } else {
            None
        };
        let (after, status) = match command {
            RankingCommand::StartVerified(book_id) => {
                let (snapshot, status) = before.start_verified_placement(book_id)?;
                (snapshot, Some(status))
            }
            RankingCommand::Answer {
                expected_state_version,
                comparison_id,
                outcome,
            } => {
                let (snapshot, status) =
                    before.answer(expected_state_version, &comparison_id, outcome)?;
                (snapshot, Some(status))
            }
            RankingCommand::ResumeExhausted => (before.resume_exhausted_skips()?, None),
        };
        if after != before {
            after.validate()?;
            if after.revision <= before.revision {
                return Err(RankingStoreError::Invalid);
            }
            persist(&mut tx, &before, &after, evidence.as_ref()).await?;
        }
        tx.commit().await?;
        Ok(RankingCommit {
            snapshot: after,
            status,
        })
    }
}

async fn load_snapshot(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    revision: i64,
) -> Result<RankingSnapshot, RankingStoreError> {
    if revision < 0 {
        return Err(RankingStoreError::Invalid);
    }
    let mut result = RankingSnapshot::empty(user_id)?;
    result.revision = revision as u64;
    let ranks = sqlx::query(
        "SELECT book_id,tier,comparisons_count FROM user_book_rank
         WHERE user_id=$1 ORDER BY tier,book_id LIMIT 501",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    if ranks.len() > MAX_RANKED_BOOKS {
        return Err(RankingStoreError::Invalid);
    }
    for row in ranks {
        let count: i64 = row.try_get("comparisons_count")?;
        if count < 0 {
            return Err(RankingStoreError::Invalid);
        }
        result.ranks.push(BookRank {
            book_id: row.try_get("book_id")?,
            tier: row.try_get("tier")?,
            comparisons_count: count as u64,
        });
    }
    let state = sqlx::query(
        "SELECT placing_book_id,low_position,high_position,state_version,placement_epoch
         FROM user_book_ranking_state WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = state {
        let state_version: i64 = row.try_get("state_version")?;
        let placement_epoch: i64 = row.try_get("placement_epoch")?;
        if state_version < 1 || placement_epoch < 1 {
            return Err(RankingStoreError::Invalid);
        }
        result.placement = Some(PlacementState {
            placing_book_id: row.try_get("placing_book_id")?,
            low_position: row.try_get("low_position")?,
            high_position: row.try_get("high_position")?,
            state_version: state_version as u64,
            placement_epoch: placement_epoch as u64,
        });
    }
    let deferred = sqlx::query(
        "SELECT book_id FROM user_book_ranking_defer
         WHERE user_id=$1 ORDER BY book_id LIMIT 501",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    if deferred.len() > MAX_RANKED_BOOKS {
        return Err(RankingStoreError::Invalid);
    }
    result.deferred_books = deferred
        .iter()
        .map(|row| row.try_get("book_id"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if let Some(state) = &result.placement {
        let skipped = sqlx::query(
            "SELECT book_a_id,book_b_id FROM user_book_comparison
             WHERE user_id=$1 AND placement_epoch=$2 AND state_version=$3
             AND outcome='skip' ORDER BY comparison_id LIMIT 501",
        )
        .bind(user_id)
        .bind(db_u64(state.placement_epoch)?)
        .bind(db_u64(state.state_version)?)
        .fetch_all(&mut **tx)
        .await?;
        if skipped.len() > MAX_RANKED_BOOKS {
            return Err(RankingStoreError::Invalid);
        }
        for row in skipped {
            result
                .skipped_pairs
                .insert((row.try_get("book_a_id")?, row.try_get("book_b_id")?));
        }
    }
    result.validate()?;
    Ok(result)
}

async fn persist(
    tx: &mut Transaction<'_, Postgres>,
    old: &RankingSnapshot,
    next: &RankingSnapshot,
    evidence: Option<&AnswerEvidence>,
) -> Result<(), RankingStoreError> {
    let user_id = old.user_id;
    if next.user_id != user_id || next.revision <= old.revision {
        return Err(RankingStoreError::Invalid);
    }
    let prior: BTreeMap<_, _> = old.ranks.iter().map(|r| (r.book_id, r)).collect();
    let current: BTreeMap<_, _> = next.ranks.iter().map(|r| (r.book_id, r)).collect();
    for book_id in prior.keys() {
        if !current.contains_key(book_id) {
            sqlx::query("DELETE FROM user_book_rank WHERE user_id=$1 AND book_id=$2")
                .bind(user_id)
                .bind(book_id)
                .execute(&mut **tx)
                .await?;
        }
    }
    for (book_id, rank) in current {
        if prior.get(&book_id).is_some_and(|value| *value == rank) {
            continue;
        }
        sqlx::query(
            "INSERT INTO user_book_rank(user_id,book_id,tier,comparisons_count)
             VALUES($1,$2,$3,$4) ON CONFLICT(user_id,book_id)
             DO UPDATE SET tier=EXCLUDED.tier,
             comparisons_count=EXCLUDED.comparisons_count,updated_at=now()",
        )
        .bind(user_id)
        .bind(book_id)
        .bind(rank.tier)
        .bind(db_u64(rank.comparisons_count)?)
        .execute(&mut **tx)
        .await?;
    }

    if old.placement != next.placement {
        sqlx::query("DELETE FROM user_book_ranking_state WHERE user_id=$1")
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
        if let Some(state) = &next.placement {
            sqlx::query(
                "INSERT INTO user_book_ranking_state(
                    user_id,placing_book_id,low_position,high_position,
                    state_version,placement_epoch
                 ) VALUES($1,$2,$3,$4,$5,$6)",
            )
            .bind(user_id)
            .bind(state.placing_book_id)
            .bind(state.low_position)
            .bind(state.high_position)
            .bind(db_u64(state.state_version)?)
            .bind(db_u64(state.placement_epoch)?)
            .execute(&mut **tx)
            .await?;
        }
    }
    for book_id in old.deferred_books.difference(&next.deferred_books) {
        sqlx::query("DELETE FROM user_book_ranking_defer WHERE user_id=$1 AND book_id=$2")
            .bind(user_id)
            .bind(book_id)
            .execute(&mut **tx)
            .await?;
    }
    for book_id in next.deferred_books.difference(&old.deferred_books) {
        sqlx::query(
            "INSERT INTO user_book_ranking_defer(user_id,book_id)
             VALUES($1,$2) ON CONFLICT(user_id,book_id) DO NOTHING",
        )
        .bind(user_id)
        .bind(book_id)
        .execute(&mut **tx)
        .await?;
    }
    if let Some(event) = evidence {
        sqlx::query(
            "INSERT INTO user_book_comparison(
                user_id,placement_epoch,state_version,comparison_id,
                book_a_id,book_b_id,outcome
             ) VALUES($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(user_id)
        .bind(event.epoch)
        .bind(event.version)
        .bind(&event.comparison_id)
        .bind(event.a)
        .bind(event.b)
        .bind(event.outcome)
        .execute(&mut **tx)
        .await?;
    }
    let wrote = sqlx::query(
        "UPDATE user_book_ranking_head
         SET revision=$2,updated_at=now() WHERE user_id=$1 AND revision=$3",
    )
    .bind(user_id)
    .bind(db_u64(next.revision)?)
    .bind(db_u64(old.revision)?)
    .execute(&mut **tx)
    .await?;
    if wrote.rows_affected() != 1 {
        return Err(RankingStoreError::Stale);
    }
    Ok(())
}
