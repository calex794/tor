//! A / B-09d: verified recipient tier, only from locked PostgreSQL facts.
//!
//! This must be invoked inside the *same* B-09 transfer transaction. FOR SHARE
//! on the user and active grant rows prevents revoking a qualifying fact
//! between eligibility verification and the committed owner change.
use crate::domain::access_level::EffectiveUserTier;
use sqlx::{Postgres, Transaction};

pub async fn verified_effective_user_tier(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
) -> Result<Option<EffectiveUserTier>, sqlx::Error> {
    if user_id <= 0 {
        return Ok(None);
    }
    let adult: Option<bool> = sqlx::query_scalar(
        "SELECT adult_enabled FROM users WHERE id=$1 FOR SHARE",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(adult) = adult else {
        return Ok(None);
    };
    let pro_adult: Option<i64> = sqlx::query_scalar(
        "SELECT user_id FROM user_level_grants
         WHERE user_id=$1 AND level='pro_adult' AND is_active
         FOR SHARE",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(Some(if pro_adult.is_some() {
        EffectiveUserTier::ProAdult
    } else if adult {
        EffectiveUserTier::Adult
    } else {
        EffectiveUserTier::Basic
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn read(pool: &PgPool, user_id: i64) -> Option<EffectiveUserTier> {
        let mut tx = pool.begin().await.unwrap();
        let tier = verified_effective_user_tier(&mut tx, user_id)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        tier
    }

    #[tokio::test]
    #[ignore = "requires isolated tor_ci PostgreSQL"]
    async fn a_b09_pg_user_tier_uses_adult_opt_in_and_active_pro_adult_grant() {
        let url = std::env::var("TEST_DATABASE_URL").expect("CI DB");
        assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
        let pool = PgPool::connect(&url).await.unwrap();
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('a-b09d-user@tor-ci.example','B09D tier','$argon2id$test')
             ON CONFLICT(email_normalized) DO UPDATE
             SET display_name=EXCLUDED.display_name RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(read(&pool, user).await, Some(EffectiveUserTier::Basic));
        assert_eq!(read(&pool, -1).await, None);
        assert_eq!(read(&pool, i64::MAX - 1).await, None);

        sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(read(&pool, user).await, Some(EffectiveUserTier::Adult));

        sqlx::query(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES($1,'pro_adult','moderation',TRUE)",
        )
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(read(&pool, user).await, Some(EffectiveUserTier::ProAdult));

        // The pro_adult grant outranks adult_enabled; it does not require the
        // lower adult flag to be independently switched on.
        sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(read(&pool, user).await, Some(EffectiveUserTier::ProAdult));
        sqlx::query(
            "UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1",
        )
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(read(&pool, user).await, Some(EffectiveUserTier::Basic));

        assert!(
            sqlx::query(
                "INSERT INTO user_level_grants(user_id,level,source,is_active)
                 VALUES($1,'adult','moderation',TRUE)",
            )
            .bind(user)
            .execute(&pool)
            .await
            .is_err(),
            "only pro_adult is a grant: adult is an opt-in flag"
        );
        assert!(
            sqlx::query(
                "UPDATE user_level_grants SET source='invitation',source_ref_id=NULL
                 WHERE user_id=$1",
            )
            .bind(user)
            .execute(&pool)
            .await
            .is_err(),
            "invitation grants need durable provenance"
        );
        pool.close().await;
    }
}
