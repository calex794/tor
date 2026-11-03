//! AB-02: atomically count failed logins against distinct identities.
//! One PostgreSQL transaction locks the IP row, then the subnet row.
//! This is independent of best-effort auth observations and never scans events.
use super::PgAccounts;

const WINDOW_SECONDS: i32 = 600;
const SOURCE_LOCK_SECONDS: i32 = 90;

/// Check only one exact verified source; a subnet cannot block a shared NAT.
/// A positive result is a bounded Retry-After in seconds.
impl PgAccounts {
    pub async fn spray_retry_after(
        &self,
        source_key: &[u8; 32],
    ) -> Result<Option<u32>, sqlx::Error> {
        let remaining: Option<i32> = sqlx::query_scalar(
            "SELECT GREATEST(0, CEIL(EXTRACT(EPOCH FROM
                (locked_until - clock_timestamp()))))::integer
             FROM auth_spray_windows
             WHERE scope='ip' AND source_key=$1 AND locked_until > clock_timestamp()",
        )
        .bind(source_key.as_slice())
        .fetch_optional(self.pool())
        .await?;
        Ok(remaining.filter(|seconds| *seconds > 0).map(|seconds| seconds as u32))
    }

    /// Persist only a confirmed failed password verification. The caller supplies
    /// keyed HMAC values, not an email/IP string or user-controlled SQL.
    ///
    /// A lock requires >=20 failures against >=12 different normalized identities
    /// in the 10-minute window. One account's password typo cannot lock a source.
    /// Subnet aggregates are never enforced: CGNAT/VPN users share a subnet.
    /// The existing per-identity and verified-peer limit remains authoritative.
    pub async fn record_spray_failure(
        &self,
        ip_key: &[u8; 32],
        subnet_key: &[u8; 32],
        target_key: &[u8; 32],
        enforce: bool,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        // Always acquire IP, then subnet: two writers can never invert this
        // lock order. A row-level lock serializes the distinct-target insert.
        for (scope, key) in [("ip", ip_key), ("subnet", subnet_key)] {
            sqlx::query(
                "INSERT INTO auth_spray_windows(scope,source_key)
                 VALUES($1,$2) ON CONFLICT (scope,source_key) DO NOTHING",
            )
            .bind(scope)
            .bind(key.as_slice())
            .execute(&mut *tx)
            .await?;
            let expired: bool = sqlx::query_scalar(
                "SELECT period_started + ($3::integer * INTERVAL '1 second')
                        <= clock_timestamp()
                 FROM auth_spray_windows
                 WHERE scope=$1 AND source_key=$2 FOR UPDATE",
            )
            .bind(scope)
            .bind(key.as_slice())
            .bind(WINDOW_SECONDS)
            .fetch_one(&mut *tx)
            .await?;
            if expired {
                sqlx::query(
                    "DELETE FROM auth_spray_targets WHERE scope=$1 AND source_key=$2",
                )
                .bind(scope)
                .bind(key.as_slice())
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE auth_spray_windows
                     SET period_started=clock_timestamp(), failed_attempts=0,
                         distinct_targets=0, locked_until=NULL,
                         touched_at=clock_timestamp()
                     WHERE scope=$1 AND source_key=$2",
                )
                .bind(scope)
                .bind(key.as_slice())
                .execute(&mut *tx)
                .await?;
            }
            let inserted = sqlx::query(
                "INSERT INTO auth_spray_targets(scope,source_key,target_key)
                 VALUES($1,$2,$3)
                 ON CONFLICT (scope,source_key,target_key) DO NOTHING",
            )
            .bind(scope)
            .bind(key.as_slice())
            .bind(target_key.as_slice())
            .execute(&mut *tx)
            .await?
            .rows_affected();
            let new_target = i32::from(inserted == 1);
            sqlx::query(
                "UPDATE auth_spray_windows
                 SET failed_attempts=failed_attempts+1,
                     distinct_targets=distinct_targets+$3,
                     touched_at=clock_timestamp(),
                     locked_until=CASE
                       WHEN scope='ip' AND $4::boolean
                        AND failed_attempts+1>=20
                        AND distinct_targets+$3>=12
                       THEN GREATEST(
                         COALESCE(locked_until,clock_timestamp()),
                         clock_timestamp()+($5::integer * INTERVAL '1 second'))
                       ELSE locked_until END
                 WHERE scope=$1 AND source_key=$2",
            )
            .bind(scope)
            .bind(key.as_slice())
            .bind(new_target)
            .bind(enforce)
            .bind(SOURCE_LOCK_SECONDS)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
