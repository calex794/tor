//! MAIL-01i: outbound recipient preflight, before any provider reservation.
//! A routable domain is not proof of mailbox existence or account ownership.
use super::{
    recipient_dns::{MailDnsResolver, MailRoute},
    recipient_dns_cache::MailDnsCache,
};
use crate::domain::email::{
    delivery_address,
    disposable::{self, DisposableMode, DisposableStatus},
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecipientRejection {
    InvalidAddress,
    DisposableDomain,
    NullMx,
    DomainNotFound,
    NoRoute,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecipientDeferral {
    DnsUnavailable,
    PolicyUnavailable,
}

/// Deliberately not Debug or serializable: contains the transport mailbox.
/// Keep this value in memory only for the current outbound attempt.
pub struct PreparedRecipient {
    transport_address: String,
    disposable: DisposableStatus,
}

impl PreparedRecipient {
    pub fn transport_address(&self) -> &str {
        &self.transport_address
    }

    pub fn disposable_status(&self) -> DisposableStatus {
        self.disposable
    }
}

/// Only Ready permits the caller to proceed to the independent source,
/// suppression, quota and cost gates. This is not a permission to send.
pub enum RecipientDecision {
    Ready(PreparedRecipient),
    Reject(RecipientRejection),
    Defer(RecipientDeferral),
}

/// Construct once per outbound runtime and share across its workers.
/// DNS is cached by canonical domain, never by full recipient address.
pub struct RecipientPreflight {
    dns: MailDnsCache,
    disposable_mode: DisposableMode,
}

impl RecipientPreflight {
    pub fn new(resolver: Arc<dyn MailDnsResolver>, disposable_mode: DisposableMode) -> Self {
        Self {
            dns: MailDnsCache::new(resolver),
            disposable_mode,
        }
    }

    pub async fn check(&self, address: &str) -> RecipientDecision {
        use RecipientDecision::{Defer, Ready, Reject};
        use RecipientRejection::*;
        let Ok(transport_address) = delivery_address(address) else {
            return Reject(InvalidAddress);
        };
        let Ok(disposable) = disposable::classify(&transport_address) else {
            return Defer(RecipientDeferral::PolicyUnavailable);
        };
        if self.disposable_mode.should_block(disposable) {
            return Reject(DisposableDomain);
        }
        let Some((_, domain)) = transport_address.rsplit_once('@') else {
            return Reject(InvalidAddress);
        };
        match self.dns.check(domain).await {
            MailRoute::ExplicitMx { .. } | MailRoute::ImplicitMx { .. } => {
                Ready(PreparedRecipient {
                    transport_address,
                    disposable,
                })
            }
            MailRoute::NullMx { .. } => Reject(NullMx),
            MailRoute::DomainNotFound { .. } => Reject(DomainNotFound),
            MailRoute::NoRoute { .. } => Reject(NoRoute),
            MailRoute::InvalidDomain => Reject(InvalidAddress),
            MailRoute::Retry => Defer(RecipientDeferral::DnsUnavailable),
        }
    }
}

#[cfg(test)]
mod tests;
