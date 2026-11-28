use super::*;
use crate::domain::quota::QuotaDecision;

struct NeverResolve;

impl DispatchSource for NeverResolve {
    fn resolve(&self, _: MailClaim) -> Resolve<'_> {
        panic!("stopped worker must never resolve a source");
    }

    fn fresh_reset_proof(&self) -> Option<Zeroizing<String>> {
        panic!("stopped worker must never mint a proof");
    }
}

#[tokio::test]
async fn stopped_and_busy_workers_do_not_even_connect_to_the_database() {
    use base64::{engine::general_purpose::STANDARD, Engine};

    // Lazy pool: no TCP connection unless a query is actually attempted.
    let stores = DispatchStores::lazy_fixture();
    let fence = Arc::new(LaunchFence::new());
    let worker = DispatchWorker::new(
        stores,
        fence.clone(),
        Arc::new(NeverResolve),
        Arc::new(ProfileMasterKey::from_base64(1, &STANDARD.encode([7; 32])).unwrap()),
        ResetOrigin::new("https://books.example").unwrap(),
    );
    assert_eq!(worker.step().await, WorkerStep::Stopped);
    let serial = fence.try_dispatch_worker().unwrap();
    assert_eq!(worker.step().await, WorkerStep::Busy);
    drop(serial);
    assert_eq!(worker.step().await, WorkerStep::Stopped);
    let (sender, receiver) = watch::channel(true);
    worker.run(receiver).await;
    drop(sender);
}

#[test]
fn retry_classification_never_retries_a_charged_or_unknown_reservation() {
    use QuotaDecision::*;
    assert_eq!(quota_action(Reserved), QuotaAction::Proceed);
    assert_eq!(quota_action(AlreadyReserved), QuotaAction::Quarantine);
    assert_eq!(quota_action(StaleClaim), QuotaAction::Quarantine);
    assert_eq!(
        quota_action(GlobalPace),
        QuotaAction::Defer(DeferReason::GlobalPace)
    );
    assert_eq!(
        quota_action(PasswordResetRecipientCooldown),
        QuotaAction::Defer(DeferReason::RecipientCooldown)
    );
    assert_eq!(
        quota_action(PasswordResetRecipientDaily),
        QuotaAction::Cancel
    );
    assert_eq!(quota_action(PasswordResetIpDaily), QuotaAction::Cancel);
    assert_eq!(quota_action(CostEmergencyStop), QuotaAction::Stop);
    assert_eq!(quota_action(DispatchNotAuthorized), QuotaAction::Stop);
    assert_eq!(quota_action(KeyMismatch), QuotaAction::Stop);
}
