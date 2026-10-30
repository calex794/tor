use super::*;
use tor_commerce::repositories::direct_exposure::{DirectGrantKind, PgDirectFirstExposure};
use tor_contracts::paid_delivery::DeliveryError;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_first_protected_grant_records_exact_pinned_purchase_once() {
    let (pool, actor, _, chapter, version) = fixture("d06_direct_grant", 1000, 300).await;
    let purchase = PgCheckout::new(pool.clone())
        .checkout_once(&command(actor, chapter, version, 300, 'X'))
        .await
        .unwrap()
        .purchase_id
        .get();
    let first = PgDirectFirstExposure::new(pool.clone());
    let id = |n| PositiveId::new(n).unwrap();
    let count = |pool: &PgPool| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM chapter_purchase_first_exposures
                 WHERE purchase_id=$1",
            )
            .bind(purchase)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(count(&pool).await, 0);
    assert_eq!(
        first
            .record(
                id(actor + 1),
                id(chapter),
                id(version),
                Some(id(purchase)),
                DirectGrantKind::Chapter
            )
            .await,
        Err(DeliveryError::Denied)
    );
    assert_eq!(
        first
            .record(
                id(actor),
                id(chapter),
                id(version + 1),
                Some(id(purchase)),
                DirectGrantKind::Chapter
            )
            .await,
        Err(DeliveryError::Denied)
    );
    let (a, b) = tokio::join!(
        first.record(
            id(actor),
            id(chapter),
            id(version),
            Some(id(purchase)),
            DirectGrantKind::Chapter
        ),
        first.record(
            id(actor),
            id(chapter),
            id(version),
            Some(id(purchase)),
            DirectGrantKind::Asset
        )
    );
    assert_eq!(a, Ok(()));
    assert_eq!(b, Ok(()));
    assert_eq!(count(&pool).await, 1);
    let original: String = sqlx::query_scalar(
        "SELECT granted_at::TEXT FROM chapter_purchase_first_exposures
         WHERE purchase_id=$1",
    )
    .bind(purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    let after: String = sqlx::query_scalar(
        "SELECT granted_at::TEXT FROM chapter_purchase_first_exposures
         WHERE purchase_id=$1",
    )
    .bind(purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(original, after);
    assert!(
        sqlx::query(
            "UPDATE chapter_purchase_first_exposures SET granted_at=now()
             WHERE purchase_id=$1"
        )
        .bind(purchase)
        .execute(&pool)
        .await
        .is_err(),
        "first-grant evidence is immutable"
    );
    assert_eq!(count(&pool).await, 1);
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES($1,$2,'refund')",
    )
    .bind(purchase)
    .bind(format!("ci:d06:revoked:{purchase}"))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        first
            .record(
                id(actor),
                id(chapter),
                id(version),
                Some(id(purchase)),
                DirectGrantKind::Chapter
            )
            .await,
        Err(DeliveryError::Denied)
    );
    assert_eq!(count(&pool).await, 1);
    assert_eq!(count(&pool).await, 1);
}
