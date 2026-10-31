//! B: persistent, explainable rate signals for book likes.
//! This is separate from RIL path diagnostics, which are explicitly not a fraud ACL.
#![forbid(unsafe_code)]

use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgBookEngagementIntegrity {
    pool: PgPool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LikeRateSignals {
    pub subject_action_1m: i64,
    pub subject_action_1h: i64,
    pub subject_any_1m: i64,
    pub target_action_10m: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LikeIntegrityDecision {
    pub signals: LikeRateSignals,
    pub risk_score: u8,
    pub risk_bucket: &'static str,
    pub recommended_action: &'static str,
    /// A risk score at or above the donor's 0.35 threshold is excluded from
    /// trusted popularity totals. The user's active like state is still saved.
    pub stats_trusted: bool,
    pub reasons: Vec<&'static str>,
}

impl PgBookEngagementIntegrity {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Record one attempt and evaluate shared 10-second buckets. All windows
    /// are read from PostgreSQL, so separate TOR processes see the same counts.
    /// The 10-second granularity can include up to 10 extra seconds at a window edge.
    pub async fn evaluate_like(
        &self,
        user_id: i64,
        book_id: i64,
        liked: bool,
    ) -> Result<LikeIntegrityDecision, sqlx::Error> {
        if user_id <= 0 || book_id <= 0 {
            return Err(sqlx::Error::Protocol(
                "invalid book engagement identity".into(),
            ));
        }
        let action = if liked { "book_like" } else { "book_unlike" };
        let user_action = scope_hash(&format!("user:{user_id}:action:{action}"));
        let user_any = scope_hash(&format!("user:{user_id}:any"));
        let target_action = scope_hash(&format!("book:{book_id}:action:{action}"));

        let mut tx = self.pool.begin().await?;
        let bucket: i64 =
            sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp()) / 10)::BIGINT")
                .fetch_one(&mut *tx)
                .await?;

        for (scope, owner_user, owner_book) in [
            (user_action, Some(user_id), None),
            (user_any, Some(user_id), None),
            (target_action, None, Some(book_id)),
        ] {
            sqlx::query(
                "INSERT INTO book_engagement_rate_buckets
                    (scope_hash,user_id,book_id,bucket_start,attempts)
                 VALUES ($1,$2,$3,to_timestamp($4::double precision * 10.0),1)
                 ON CONFLICT (scope_hash,bucket_start) DO UPDATE
                    SET attempts=book_engagement_rate_buckets.attempts+1",
            )
            .bind(scope.as_slice())
            .bind(owner_user)
            .bind(owner_book)
            .bind(bucket)
            .execute(&mut *tx)
            .await?;
        }

        let row = sqlx::query(
            "SELECT
                COALESCE((SELECT SUM(attempts) FROM book_engagement_rate_buckets
                    WHERE scope_hash=$1
                      AND bucket_start >= clock_timestamp()-INTERVAL '70 seconds'),0)::BIGINT
                    AS subject_action_1m,
                COALESCE((SELECT SUM(attempts) FROM book_engagement_rate_buckets
                    WHERE scope_hash=$1
                      AND bucket_start >= clock_timestamp()-INTERVAL '3610 seconds'),0)::BIGINT
                    AS subject_action_1h,
                COALESCE((SELECT SUM(attempts) FROM book_engagement_rate_buckets
                    WHERE scope_hash=$2
                      AND bucket_start >= clock_timestamp()-INTERVAL '70 seconds'),0)::BIGINT
                    AS subject_any_1m,
                COALESCE((SELECT SUM(attempts) FROM book_engagement_rate_buckets
                    WHERE scope_hash=$3
                      AND bucket_start >= clock_timestamp()-INTERVAL '610 seconds'),0)::BIGINT
                    AS target_action_10m",
        )
        .bind(user_action.as_slice())
        .bind(user_any.as_slice())
        .bind(target_action.as_slice())
        .fetch_one(&mut *tx)
        .await?;
        let signals = LikeRateSignals {
            subject_action_1m: row.try_get("subject_action_1m")?,
            subject_action_1h: row.try_get("subject_action_1h")?,
            subject_any_1m: row.try_get("subject_any_1m")?,
            target_action_10m: row.try_get("target_action_10m")?,
        };
        let decision = decide_like_signals(signals);
        if decision.risk_score >= 35 {
            let reasons = decision
                .reasons
                .iter()
                .map(|reason| (*reason).to_owned())
                .collect::<Vec<_>>();
            let final_action = if decision.recommended_action == "allow" {
                "allow"
            } else {
                // Donor defaults to observe-only. The existing 20/min hard
                // limiter remains the only automatic request throttle here.
                "observe"
            };
            sqlx::query(
                "INSERT INTO book_engagement_integrity_events
                    (user_id,book_id,action,subject_action_1m,subject_action_1h,
                     subject_any_1m,target_action_10m,risk_score,risk_bucket,
                     recommended_action,decision,stats_trusted,reasons)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
            )
            .bind(user_id)
            .bind(book_id)
            .bind(action)
            .bind(signals.subject_action_1m)
            .bind(signals.subject_action_1h)
            .bind(signals.subject_any_1m)
            .bind(signals.target_action_10m)
            .bind(i16::from(decision.risk_score))
            .bind(decision.risk_bucket)
            .bind(decision.recommended_action)
            .bind(final_action)
            .bind(decision.stats_trusted)
            .bind(reasons)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(decision)
    }

    /// Record the hard 20/minute refusal without an additional rate-bucket
    /// evaluation. Only one refusal per user/book/action/minute is persisted.
    /// A hard-limit decision is independent of the fraud risk score.
    pub async fn record_hard_limit(
        &self,
        user_id: i64,
        book_id: i64,
        liked: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO book_engagement_integrity_events
                (user_id,book_id,action,subject_action_1m,subject_action_1h,
                 subject_any_1m,target_action_10m,risk_score,risk_bucket,
                 recommended_action,decision,stats_trusted,reasons,hard_limit_minute)
             VALUES ($1,$2,$3,0,0,0,0,0,'low','throttle','throttle',FALSE,
                     ARRAY['hard_limit_20_per_minute']::TEXT[],
                     date_trunc('minute', clock_timestamp()))
             ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .bind(book_id)
        .bind(if liked { "book_like" } else { "book_unlike" })
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn scope_hash(scope: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"tor-book-engagement-integrity-v1:");
    hasher.update(scope.as_bytes());
    hasher.finalize().into()
}

fn decide_like_signals(signals: LikeRateSignals) -> LikeIntegrityDecision {
    let mut score = 0_u16;
    let mut reasons = Vec::new();
    if signals.subject_action_1m > 30 {
        score += 45;
        reasons.push("subject_action_rate_1m");
    }
    if signals.subject_action_1h > 300 {
        score += 25;
        reasons.push("subject_action_rate_1h");
    }
    if signals.subject_any_1m > 60 {
        score += 25;
        reasons.push("subject_any_rate_1m");
    }
    if signals.target_action_10m > 500 {
        score += 20;
        reasons.push("target_action_rate_10m");
    }
    let risk_score = u8::try_from(score.min(100)).expect("score is capped at 100");
    let risk_bucket = if risk_score >= 90 {
        "critical"
    } else if risk_score >= 70 {
        "high"
    } else if risk_score >= 55 {
        "medium"
    } else if risk_score >= 35 {
        "watch"
    } else {
        "low"
    };
    let recommended_action = if risk_score >= 70 {
        "throttle"
    } else if risk_score >= 45 {
        "observe"
    } else {
        "allow"
    };
    LikeIntegrityDecision {
        signals,
        risk_score,
        risk_bucket,
        recommended_action,
        stats_trusted: risk_score < 35,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::{decide_like_signals, LikeRateSignals};

    #[test]
    fn b_like_integrity_uses_donor_rate_weights_and_trusted_threshold() {
        let low = decide_like_signals(LikeRateSignals::default());
        assert_eq!(low.risk_score, 0);
        assert!(low.stats_trusted);
        assert_eq!(low.recommended_action, "allow");

        let watch = decide_like_signals(LikeRateSignals {
            subject_action_1m: 31,
            ..LikeRateSignals::default()
        });
        assert_eq!(watch.risk_score, 45);
        assert_eq!(watch.risk_bucket, "watch");
        assert_eq!(watch.recommended_action, "observe");
        assert!(!watch.stats_trusted);

        let throttle = decide_like_signals(LikeRateSignals {
            subject_action_1m: 31,
            subject_action_1h: 301,
            subject_any_1m: 61,
            target_action_10m: 501,
        });
        assert_eq!(throttle.risk_score, 100);
        assert_eq!(throttle.risk_bucket, "critical");
        assert_eq!(throttle.recommended_action, "throttle");
        assert!(!throttle.stats_trusted);
    }
}
