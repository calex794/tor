//! Opt-in bounded security telemetry; no dependency of successful login on storage.
//! This is diagnostic only: the existing PostgreSQL rate limiter is authoritative.
use super::AuthState;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, OnceLock,
};
use tokio::sync::Semaphore;
use tor_accounts::{
    domain::{
        access_audit::LoginAuditContext,
        auth_observation::{AuthAttemptObservation, AuthAttemptOutcome},
    },
    repositories::PgAccounts,
    services::auth_observation::AuthObservationSigner,
};

pub(super) use tor_accounts::domain::auth_observation::AuthAttemptOutcome as Outcome;

static IN_FLIGHT: OnceLock<Arc<Semaphore>> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);

pub(super) fn dropped_count() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

pub(super) struct RequestObservation {
    accounts: PgAccounts,
    signer: Option<AuthObservationSigner>,
    normalized_email: Option<String>,
    source: LoginAuditContext,
}

impl RequestObservation {
    pub(super) fn new(state: &AuthState, email: &str, source: LoginAuditContext) -> Self {
        // Fail closed for IDENTIFIED telemetry if no independent secret is set.
        // Auth itself continues normally; disabled telemetry is observable.
        let signer = std::env::var("TOR_AUTH_OBSERVATION_KEY")
            .ok()
            .and_then(|key| AuthObservationSigner::from_hex(&key));
        let normalized_email = tor_accounts::domain::normalize_email(email).ok();
        Self {
            accounts: state.accounts.clone(),
            signer,
            normalized_email,
            source,
        }
    }

    pub(super) fn report(&mut self, outcome: AuthAttemptOutcome, user_id: Option<i64>) {
        let Some(signer) = self.signer.as_ref() else {
            return;
        };
        let target_key = self
            .normalized_email
            .as_deref()
            .map(|email| signer.target(email));
        let (source_key, subnet_key) = match self.source.verified_ip {
            Some(ip) => {
                let (source, subnet) = signer.source(ip);
                (Some(source), Some(subnet))
            }
            None => (None, None),
        };
        let event = AuthAttemptObservation {
            target_key,
            user_id,
            source_key,
            subnet_key,
            agent: self.source.agent_family,
            outcome,
        };
        let limiter = IN_FLIGHT
            .get_or_init(|| Arc::new(Semaphore::new(8)))
            .clone();
        let Ok(permit) = limiter.try_acquire_owned() else {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let accounts = self.accounts.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if accounts.record_auth_observation(&event).await.is_err() {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        });
    }
}
