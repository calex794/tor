use super::*;
use sha2::{Digest, Sha256};
use tor_commerce::repositories::{
    refund_approvals::PgDirectRefundApprovals,
    refund_decisions::{
        rejection_step_up_payload_digest, DirectRefundRejectError, PgDirectRefundDecisions,
    },
};

async fn rejection_proof(
    f: &d06_refund_approval::RefundFixture,
    reason: &str,
    scope: &str,
) -> ([u8; 32], String) {
    // The A module independently tests password verification. D exercises
    // the exact one-use proof written by A in a disposable PostgreSQL session.
    let session_digest: [u8; 32] =
        Sha256::digest(format!("d06:reject:{}", f.moderator).as_bytes()).into();
    let token = format!("{}8", "R".repeat(42));
    let proof = Sha256::digest(token.as_bytes());
    let payload = rejection_step_up_payload_digest(f.moderator, f.request, 1, reason);
    sqlx::query(
        "INSERT INTO sessions(
             user_id,token_hash,expires_at,step_up_proof_hash,step_up_scope,
             step_up_resource_id,step_up_revision,step_up_payload_digest,
             step_up_issued_at,step_up_expires_at)
         VALUES($1,$2,statement_timestamp()+INTERVAL '1 hour',
             $3,$4,$5,1,$6,
             statement_timestamp(),statement_timestamp()+INTERVAL '5 minutes')",
    )
    .bind(f.moderator)
    .bind(session_digest.as_slice())
    .bind(proof.as_slice())
    .bind(scope)
    .bind(f.request)
    .bind(payload.as_slice())
    .execute(&f.pool)
    .await
    .unwrap();
    (session_digest, token)
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_refund_reject_once_no_wallet_change_and_no_approval_replay() {
    let f = d06_refund_approval::d06_approval_fixture("d06_reject_protected", 86400).await;
    let service = PgDirectRefundDecisions::new(f.pool.clone());
    let (session, token) =
        rejection_proof(&f, "policy conditions not met", "finance.refund.reject.v1").await;

    assert_eq!(
        service
            .reject_once(
                f.buyer,
                &session,
                &token,
                f.request,
                1,
                "policy conditions not met"
            )
            .await,
        Err(DirectRefundRejectError::Forbidden)
    );
    assert_eq!(
        service
            .reject_once(
                f.moderator,
                &session,
                &token,
                f.request,
                1,
                "different reason"
            )
            .await,
        Err(DirectRefundRejectError::InvalidStepUp)
    );
    assert_eq!(
        service
            .reject_once(
                f.moderator,
                &session,
                "bad",
                f.request,
                1,
                "policy conditions not met"
            )
            .await,
        Err(DirectRefundRejectError::InvalidRequest)
    );
    let first = service
        .reject_once(
            f.moderator,
            &session,
            &token,
            f.request,
            1,
            "policy conditions not met",
        )
        .await
        .unwrap();
    assert_eq!(first.request_id, f.request);
    assert_eq!(first.revision, 2);
    assert!(!first.replayed);
    let replay = service
        .reject_once(
            f.moderator,
            &session,
            &token,
            f.request,
            1,
            "policy conditions not met",
        )
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.revision, 2);
    assert_eq!(
        service
            .reject_once(
                f.moderator,
                &session,
                &token,
                f.request,
                1,
                "another reason"
            )
            .await,
        Err(DirectRefundRejectError::Conflict)
    );

    let state: (String, i64) =
        sqlx::query_as("SELECT state,revision FROM finance_direct_refund_requests WHERE id=$1")
            .bind(f.request)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(state, ("rejected".to_owned(), 2));
    let balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(f.buyer)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(balance, 700);
    for table in [
        "finance_direct_refund_postings",
        "chapter_purchase_revocations",
    ] {
        // These exact names are test-owned known schema identifiers.
        let query = format!("SELECT COUNT(*) FROM {table} WHERE purchase_id=$1");
        let count: i64 = sqlx::query_scalar(&query)
            .bind(f.purchase)
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} must remain untouched");
    }
    let (approval_session, approval_token) =
        d06_refund_approval::issue_refund_step_up(&f, "approved").await;
    let rejected_cannot_approve = PgDirectRefundApprovals::new(f.pool.clone())
        .approve_once(
            f.moderator,
            &approval_session,
            &approval_token,
            f.request,
            1,
            "approved",
        )
        .await;
    assert!(rejected_cannot_approve.is_err());
    d06_refund_approval::d06_approval_cleanup(f).await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_refund_reject_requires_live_admin_revision_and_separate_scope() {
    let f = d06_refund_approval::d06_approval_fixture("d06_reject_denied", 86400).await;
    let service = PgDirectRefundDecisions::new(f.pool.clone());
    let (session, token) = rejection_proof(&f, "not refundable", "finance.refund.approve.v1").await;
    assert_eq!(
        service
            .reject_once(
                f.moderator,
                &session,
                &token,
                f.request,
                1,
                "not refundable"
            )
            .await,
        Err(DirectRefundRejectError::InvalidStepUp)
    );
    assert_eq!(
        service
            .reject_once(
                f.moderator,
                &session,
                &token,
                f.request,
                2,
                "not refundable"
            )
            .await,
        Err(DirectRefundRejectError::RevisionConflict)
    );
    sqlx::query(
        "UPDATE account_roles SET is_active=FALSE
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(f.moderator)
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        service
            .reject_once(
                f.moderator,
                &session,
                &token,
                f.request,
                1,
                "not refundable"
            )
            .await,
        Err(DirectRefundRejectError::Forbidden)
    );
    let state: String =
        sqlx::query_scalar("SELECT state FROM finance_direct_refund_requests WHERE id=$1")
            .bind(f.request)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(state, "pending");
    d06_refund_approval::d06_approval_cleanup(f).await;
}
