//! REC-POLICY-01B: persisted organic ratios, authenticated CAS and immutable audit.
//! Uses the disposable PostgreSQL 18 fixture; never targets the live TOR database.
use super::*;

async fn set_shares(
    pool: &PgPool,
    digest: &[u8],
    expected: i64,
    ratios: [i32; 5],
) -> Result<i64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('tor.rec_share_session',encode($1::bytea,'hex'),true)")
        .bind(digest)
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query_scalar::<_, i64>(
        "SELECT rec_shelf_share_update($1,$2,$3,$4,$5,$6)",
    )
    .bind(expected)
    .bind(ratios[0])
    .bind(ratios[1])
    .bind(ratios[2])
    .bind(ratios[3])
    .bind(ratios[4])
    .fetch_one(&mut *tx)
    .await;
    match result {
        Ok(revision) => {
            tx.commit().await?;
            Ok(revision)
        }
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

fn db_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|db| db.code())
        .map(|code| code.into_owned())
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL with REC-POLICY-01B migration"]
async fn shelf_shares_cas_enforces_admin_and_records_every_revision() {
    let f = fixture("share-policy").await;
    let actor = f.actors[0];
    let digest = tor_recommendations::domain::feedback::token_digest(&new_ticket().unwrap())
        .unwrap();

    sqlx::query("INSERT INTO account_roles(user_id,role) VALUES($1,'administrator')")
        .bind(actor)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sessions(token_hash,user_id,expires_at)
         VALUES($1,$2,clock_timestamp()+interval '1 hour')",
    )
    .bind(digest.as_slice())
    .bind(actor)
    .execute(&f.pool)
    .await
    .unwrap();

    let (initial, author, team, other, no_team_author, no_team_other):
        (i64, i16, i16, i16, i16, i16) = sqlx::query_as(
        "SELECT revision,author_pct,teammate_pct,other_authors_pct,
                no_team_author_pct,no_team_other_authors_pct
         FROM rec_shelf_share_policy WHERE singleton",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!([author, team, other], [50, 40, 10]);
    assert_eq!([no_team_author, no_team_other], [70, 30]);

    // Neither a caller-provided user ID nor direct SQL is an admin capability.
    let unauthorized = sqlx::query_scalar::<_, i64>(
        "SELECT rec_shelf_share_update($1,40,30,30,60,40)",
    )
    .bind(initial)
    .fetch_one(&f.pool)
    .await
    .unwrap_err();
    assert_eq!(db_code(&unauthorized).as_deref(), Some("42501"));
    let direct = sqlx::query("UPDATE rec_shelf_share_policy SET revision=revision+1")
        .execute(&f.pool)
        .await
        .unwrap_err();
    assert_eq!(db_code(&direct).as_deref(), Some("42501"));

    for invalid in [[50, 40, 20, 70, 30], [-1, 51, 50, 70, 30], [50, 40, 10, 60, 39]] {
        let error = set_shares(&f.pool, digest.as_slice(), initial, invalid)
            .await
            .unwrap_err();
        assert_eq!(db_code(&error).as_deref(), Some("22023"));
    }
    let unchanged: i64 =
        sqlx::query_scalar("SELECT revision FROM rec_shelf_share_policy WHERE singleton")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(unchanged, initial);

    // First admin update persists a complete pair of percentage distributions.
    let first = set_shares(&f.pool, digest.as_slice(), initial, [40, 30, 30, 60, 40])
        .await
        .unwrap();
    assert_eq!(first, initial + 1);
    let saved: (i16, i16, i16, i16, i16) = sqlx::query_as(
        "SELECT author_pct,teammate_pct,other_authors_pct,
                no_team_author_pct,no_team_other_authors_pct
         FROM rec_shelf_share_policy WHERE singleton",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(saved, (40, 30, 30, 60, 40));

    let stale = set_shares(&f.pool, digest.as_slice(), initial, [50, 40, 10, 70, 30])
        .await
        .unwrap_err();
    assert_eq!(db_code(&stale).as_deref(), Some("P4090"));

    // Two administrators holding the same revision cannot both win a CAS.
    // Using the same valid admin session is sufficient to test the row lock.
    let (left, right) = tokio::join!(
        set_shares(&f.pool, digest.as_slice(), first, [20, 40, 40, 60, 40]),
        set_shares(&f.pool, digest.as_slice(), first, [40, 40, 20, 60, 40])
    );
    assert_eq!(left.is_ok() as usize + right.is_ok() as usize, 1);
    for result in [left, right] {
        if let Err(error) = result {
            assert_eq!(db_code(&error).as_deref(), Some("P4090"));
        }
    }

    let (revision, auditor, changes): (i64, i64, i64) = sqlx::query_as(
        "SELECT p.revision,
                (SELECT actor_user_id FROM rec_shelf_share_policy_audit
                 WHERE revision=p.revision),
                (SELECT count(*) FROM rec_shelf_share_policy_audit
                 WHERE revision>$1)
         FROM rec_shelf_share_policy p WHERE singleton",
    )
    .bind(initial)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(revision, initial + 2);
    assert_eq!(auditor, actor);
    assert_eq!(changes, 2);

    let audit_delete = sqlx::query(
        "DELETE FROM rec_shelf_share_policy_audit WHERE revision=$1",
    )
    .bind(revision)
    .execute(&f.pool)
    .await
    .unwrap_err();
    assert_eq!(db_code(&audit_delete).as_deref(), Some("42501"));

    sqlx::query(
        "UPDATE account_roles SET is_active=false WHERE user_id=$1 AND role='administrator'",
    )
    .bind(actor)
    .execute(&f.pool)
    .await
    .unwrap();
    let revoked_role = set_shares(&f.pool, digest.as_slice(), revision, [50, 40, 10, 70, 30])
        .await
        .unwrap_err();
    assert_eq!(db_code(&revoked_role).as_deref(), Some("42501"));
    sqlx::query(
        "UPDATE account_roles SET is_active=true WHERE user_id=$1 AND role='administrator'",
    )
    .bind(actor)
    .execute(&f.pool)
    .await
    .unwrap();

    sqlx::query("UPDATE sessions SET revoked_at=clock_timestamp() WHERE token_hash=$1")
        .bind(digest.as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    let revoked_session = set_shares(&f.pool, digest.as_slice(), revision, [50, 40, 10, 70, 30])
        .await
        .unwrap_err();
    assert_eq!(db_code(&revoked_session).as_deref(), Some("42501"));
    sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE token_hash=$1")
        .bind(digest.as_slice())
        .execute(&f.pool)
        .await
        .unwrap();

    let final_revision =
        set_shares(&f.pool, digest.as_slice(), revision, [50, 40, 10, 70, 30])
            .await
            .unwrap();
    assert_eq!(final_revision, initial + 3);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rec_shelf_share_policy_audit
         WHERE revision>$1 AND revision<=$2",
    )
    .bind(initial)
    .bind(final_revision)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(count, 3);

    sqlx::query("DELETE FROM sessions WHERE token_hash=$1")
        .bind(digest.as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM account_roles WHERE user_id=$1 AND role='administrator'")
        .bind(actor)
        .execute(&f.pool)
        .await
        .unwrap();
    f.cleanup().await;
}
