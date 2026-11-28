//! Composition adapter: reuse accounts' recipient policy and cached system DNS.
use std::sync::Arc;
use tor_accounts::{
    domain::email::disposable::DisposableMode,
    repositories::recipient_dns::SystemMailDns,
    services::{
        recipient_dns::MailDnsResolver,
        recipient_preflight::{RecipientDecision, RecipientPreflight},
    },
};
use tor_mail::services::dispatch::{DispatchPreflight, PreflightFailure};

/// Construct once per runtime. No automatic DNS lookup or SMTP at startup.
pub struct MailDispatchPreflight {
    inner: RecipientPreflight,
}

impl MailDispatchPreflight {
    #[cfg(any(unix, target_os = "windows"))]
    pub fn from_system() -> Result<Self, &'static str> {
        Ok(Self::new(Arc::new(SystemMailDns::from_system()?)))
    }

    pub fn new(resolver: Arc<dyn MailDnsResolver>) -> Self {
        Self {
            inner: RecipientPreflight::new(resolver, DisposableMode::Observe),
        }
    }
}

impl DispatchPreflight for MailDispatchPreflight {
    async fn check(&self, recipient: &str) -> Result<(), PreflightFailure> {
        // The source supplies the canonical mailbox before quota hashing.
        // Never silently send to a differently normalized address.
        match self.inner.check(recipient).await {
            RecipientDecision::Ready(prepared) if prepared.transport_address() == recipient => {
                Ok(())
            }
            RecipientDecision::Ready(_) | RecipientDecision::Reject(_) => {
                Err(PreflightFailure::InvalidRecipient)
            }
            RecipientDecision::Defer(_) => Err(PreflightFailure::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tor_accounts::services::recipient_dns::{DnsAnswer, Lookup, MxRecord};

    struct FakeDns {
        answer: DnsAnswer<Vec<MxRecord>>,
        calls: AtomicUsize,
    }

    impl MailDnsResolver for FakeDns {
        fn mx<'a>(&'a self, _: &'a str) -> Lookup<'a, Vec<MxRecord>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { self.answer.clone() })
        }

        fn a<'a>(&'a self, _: &'a str) -> Lookup<'a, ()> {
            Box::pin(async { DnsAnswer::TemporaryFailure })
        }

        fn aaaa<'a>(&'a self, _: &'a str) -> Lookup<'a, ()> {
            Box::pin(async { DnsAnswer::TemporaryFailure })
        }
    }

    #[tokio::test]
    async fn mail_dispatch_reuses_domain_cache_and_rejects_noncanonical_source() {
        let dns = Arc::new(FakeDns {
            answer: DnsAnswer::Records {
                value: vec![MxRecord {
                    preference: 10,
                    exchange: "mx.example.com.".into(),
                }],
                ttl: 60,
            },
            calls: AtomicUsize::new(0),
        });
        let preflight = MailDispatchPreflight::new(dns.clone());
        assert_eq!(preflight.check("a@example.com").await, Ok(()));
        assert_eq!(preflight.check("b@example.com").await, Ok(()));
        assert_eq!(dns.calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            preflight.check("bad@@example.com").await,
            Err(PreflightFailure::InvalidRecipient)
        );
        assert_eq!(dns.calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            preflight.check("a@пример.рф").await,
            Err(PreflightFailure::InvalidRecipient)
        );
    }

    #[tokio::test]
    async fn mail_dispatch_distinguishes_negative_dns_from_unavailable() {
        for (answer, expected) in [
            (
                DnsAnswer::NxDomain { ttl: 60 },
                PreflightFailure::InvalidRecipient,
            ),
            (
                DnsAnswer::TemporaryFailure,
                PreflightFailure::Unavailable,
            ),
        ] {
            let preflight = MailDispatchPreflight::new(Arc::new(FakeDns {
                answer,
                calls: AtomicUsize::new(0),
            }));
            assert_eq!(preflight.check("a@example.com").await, Err(expected));
        }
    }
}
