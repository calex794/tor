//! Staged reset request ingress and durable reference handoff; no email I/O.
use std::{net::IpAddr, sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tor_accounts::{domain::email::delivery_address, repositories::PgAccounts};
use tor_mail::{domain::quota::identity::QuotaKeySigner, repositories::PgMailQueue};
use zeroize::Zeroizing;

const DATABASE_BUDGET: Duration = Duration::from_secs(3);

/// Even malformed nonzero selections suppress the legacy path.
pub(super) fn queue_selected() -> bool {
    std::env::var_os("TOR_PASSWORD_RESET_QUEUE_ENABLED").is_some_and(|value| value != "0")
}

#[derive(Clone)]
pub(super) struct ResetQueue {
    signer: Arc<QuotaKeySigner>,
    permits: Arc<Semaphore>,
}

impl ResetQueue {
    pub(super) fn from_env() -> Option<Self> {
        if std::env::var("TOR_PASSWORD_RESET_ENABLED").as_deref() != Ok("1")
            || std::env::var("TOR_PASSWORD_RESET_QUEUE_ENABLED").as_deref() != Ok("1")
        {
            return None;
        }
        let key = Zeroizing::new(std::env::var("TOR_MAIL_QUOTA_KEY").ok()?);
        Some(Self {
            signer: Arc::new(QuotaKeySigner::from_hex(key.as_str())?),
            permits: Arc::new(Semaphore::new(4)),
        })
    }

    pub(super) fn reserve(&self) -> Option<OwnedSemaphorePermit> {
        self.permits.clone().try_acquire_owned().ok()
    }

    pub(super) async fn capture(
        &self,
        accounts: PgAccounts,
        email: String,
        verified_ip: Option<IpAddr>,
        _permit: OwnedSemaphorePermit,
    ) {
        let Ok(address) = delivery_address(&email) else {
            return;
        };
        let Ok(keys) = self.signer.keys(&address, verified_ip) else {
            return;
        };
        // Do not distinguish a missing account, throttled source or successful
        // source in the HTTP response, or log identifiers on failures.
        if !matches!(
            tokio::time::timeout(
                DATABASE_BUDGET,
                accounts.capture_password_reset_mail_request(&email, keys.password_reset_source()),
            )
            .await,
            Ok(Ok(_))
        ) {
            eprintln!("TOR_RESET_SOURCE_UNAVAILABLE");
        }
    }
}

async fn handoff_once(accounts: &PgAccounts, queue: &PgMailQueue) -> Result<(), ()> {
    let pending = accounts
        .pending_password_reset_mail_intents(100)
        .await
        .map_err(|_| ())?;
    for intent in pending {
        // The worker still must re-resolve the source after claim and
        // immediately before proof issuance. This is only a durable reference.
        if accounts
            .resolve_password_reset_mail_request(intent)
            .await
            .map_err(|_| ())?
            .is_none()
        {
            continue;
        }
        // Both Ok(true) and Ok(false) prove the unique reference exists;
        // errors/timeouts do not acknowledge the source.
        queue.enqueue(intent).await.map_err(|_| ())?;
        accounts
            .mark_password_reset_mail_enqueued(intent)
            .await
            .map_err(|_| ())?;
    }
    Ok(())
}

/// Explicit server-entry-point task; no worker is created by Router construction.
/// No transport, quota reservation, proof issuance or automatic retry of SMTP.
pub fn start_handoff(accounts: Option<PgAccounts>) -> Option<tokio::task::JoinHandle<()>> {
    if std::env::var("TOR_AUTH_ENABLED").as_deref() != Ok("1") {
        return None;
    }
    let accounts = accounts?;
    ResetQueue::from_env()?;
    let queue = PgMailQueue::new(accounts.pool().clone());
    Some(tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(5));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            if !matches!(
                tokio::time::timeout(DATABASE_BUDGET, handoff_once(&accounts, &queue)).await,
                Ok(Ok(()))
            ) {
                eprintln!("TOR_RESET_HANDOFF_UNAVAILABLE");
            }
        }
    }))
}
