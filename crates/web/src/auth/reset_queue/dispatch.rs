//! Account-owned resolver for the common outbound dispatcher.
//! The site passes one shared preflight cache and launch fence; no new
//! environment configuration or independent SMTP worker is created here.
use super::PgAccounts;
use std::sync::Arc;
use tor_accounts::services::{
    recipient_preflight::{RecipientDecision, RecipientPreflight},
    security::generate_session_token,
};
use tor_contracts::mail::MailPurpose;
use tor_mail::{
    domain::{
        dispatch::DeferReason, postbox_profile::ProfileMasterKey, quota::identity::QuotaKeySigner,
        MailClaim,
    },
    repositories::dispatch::DispatchStores,
    services::{
        dispatch::{DispatchSource, DispatchWorker, Resolve, ResolvedReset, SourceDecision},
        launch::LaunchFence,
        postbox::ResetOrigin,
    },
};
use zeroize::Zeroizing;

struct ResetSource {
    accounts: PgAccounts,
    signer: Arc<QuotaKeySigner>,
    preflight: Arc<RecipientPreflight>,
}

impl DispatchSource for ResetSource {
    fn resolve(&self, claim: MailClaim) -> Resolve<'_> {
        Box::pin(async move {
            if claim.intent.purpose != MailPurpose::PasswordReset {
                return SourceDecision::Cancel;
            }
            let (evidence, address) = match self
                .accounts
                .resolve_password_reset_mail_request(claim.intent)
                .await
            {
                Ok(Some(value)) => value,
                Ok(None) => return SourceDecision::Cancel,
                Err(_) => return SourceDecision::Unavailable,
            };
            let ready = match self.preflight.check(&address).await {
                RecipientDecision::Ready(value) => value,
                RecipientDecision::Reject(_) => return SourceDecision::Cancel,
                RecipientDecision::Defer(_) => {
                    return SourceDecision::Defer(DeferReason::DnsUnavailable);
                }
            };
            let transport_address = ready.transport_address();
            let Ok(keys) =
                self.signer
                    .keys_for_password_reset(claim.intent, transport_address, evidence)
            else {
                // A changed/missing key or mismatched source cannot authorize
                // delivery. The worker must stop for operator review.
                return SourceDecision::Unavailable;
            };
            SourceDecision::PasswordReset(ResolvedReset::new(transport_address.to_owned(), keys))
        })
    }

    fn fresh_reset_proof(&self) -> Option<Zeroizing<String>> {
        Some(Zeroizing::new(generate_session_token()))
    }
}

/// Composition seam for the single future server-owned mail runtime.
/// Constructing this does not start a task or contact Postbox.
pub fn build_password_reset_dispatch(
    accounts: PgAccounts,
    signer: Arc<QuotaKeySigner>,
    preflight: Arc<RecipientPreflight>,
    fence: Arc<LaunchFence>,
    master: Arc<ProfileMasterKey>,
    origin: ResetOrigin,
) -> DispatchWorker {
    let pool = accounts.pool().clone();
    let source = Arc::new(ResetSource {
        accounts,
        signer,
        preflight,
    });
    DispatchWorker::new(DispatchStores::new(pool), fence, source, master, origin)
}
