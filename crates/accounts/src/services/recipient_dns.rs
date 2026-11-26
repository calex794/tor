//! MAIL-01b: bounded DNS routing policy, independent of SMTP or HTTP.
//! A published mail route is not proof that a mailbox exists.
use std::{future::Future, pin::Pin, time::Duration};

const DNS_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MxRecord {
    pub preference: u16,
    pub exchange: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DnsAnswer<T> {
    Records { value: T, ttl: u32 },
    NoData { ttl: u32 },
    NxDomain { ttl: u32 },
    TemporaryFailure,
}

pub type Lookup<'a, T> = Pin<Box<dyn Future<Output = DnsAnswer<T>> + Send + 'a>>;

/// An adapter must distinguish authoritative negative answers from timeouts,
/// SERVFAIL, REFUSED, malformed/truncated responses and resolver outages.
/// Address records mean at least one A or AAAA record of the requested type.
pub trait MailDnsResolver: Send + Sync {
    fn mx<'a>(&'a self, domain: &'a str) -> Lookup<'a, Vec<MxRecord>>;
    fn a<'a>(&'a self, domain: &'a str) -> Lookup<'a, ()>;
    fn aaaa<'a>(&'a self, domain: &'a str) -> Lookup<'a, ()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailRoute {
    ExplicitMx { ttl: u32 },
    ImplicitMx { ttl: u32 },
    NullMx { ttl: u32 },
    DomainNotFound { ttl: u32 },
    NoRoute { ttl: u32 },
    Retry,
    InvalidDomain,
}

/// Only the canonical ASCII transport domain is accepted. Never send the
/// mailbox local part to a resolver. The future adapter must use an absolute
/// DNS name, without the operating system's search suffix expansion.
fn public_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        && domain
            .rsplit('.')
            .next()
            .is_some_and(|suffix| !suffix.bytes().all(|b| b.is_ascii_digit()))
}

async fn bounded<T>(lookup: Lookup<'_, T>) -> DnsAnswer<T> {
    tokio::time::timeout(DNS_TIMEOUT, lookup)
        .await
        .unwrap_or(DnsAnswer::TemporaryFailure)
}

/// At most one MX lookup, followed only on NODATA by parallel A/AAAA queries.
/// An explicit MX never falls back to the domain's web server. DNS errors
/// defer delivery rather than consume an SMTP quota or permanently reject.
pub async fn check_mail_domain(resolver: &dyn MailDnsResolver, domain: &str) -> MailRoute {
    if !public_domain(domain) {
        return MailRoute::InvalidDomain;
    }

    match bounded(resolver.mx(domain)).await {
        DnsAnswer::Records { value, ttl } => explicit_route(&value, ttl),
        DnsAnswer::NxDomain { ttl } => MailRoute::DomainNotFound { ttl },
        DnsAnswer::TemporaryFailure => MailRoute::Retry,
        DnsAnswer::NoData { ttl } => {
            let (a, aaaa) =
                tokio::join!(bounded(resolver.a(domain)), bounded(resolver.aaaa(domain)));
            implicit_route(ttl, a, aaaa)
        }
    }
}

fn explicit_route(records: &[MxRecord], ttl: u32) -> MailRoute {
    if records.len() == 1 && records[0].preference == 0 && records[0].exchange == "." {
        return MailRoute::NullMx { ttl };
    }
    // A mixed or malformed Null MX RRset is not a definitive negative answer.
    // Nor is an empty RRset a positive answer.
    if records.is_empty()
        || records.iter().any(|record| {
            let target = record
                .exchange
                .strip_suffix('.')
                .unwrap_or(&record.exchange);
            !public_domain(target)
        })
    {
        return MailRoute::Retry;
    }
    MailRoute::ExplicitMx { ttl }
}

fn implicit_route(mx_ttl: u32, a: DnsAnswer<()>, aaaa: DnsAnswer<()>) -> MailRoute {
    use DnsAnswer::{NoData, NxDomain, Records};
    // An NXDOMAIN contradicts an existing domain's NODATA or positive answer.
    // The DNS data may have changed between lookups; retry rather than guess.
    if matches!(a, NxDomain { .. }) || matches!(aaaa, NxDomain { .. }) {
        return MailRoute::Retry;
    }
    match (a, aaaa) {
        (Records { ttl: v4, .. }, Records { ttl: v6, .. }) => MailRoute::ImplicitMx {
            ttl: mx_ttl.min(v4).min(v6),
        },
        (Records { ttl, .. }, _) | (_, Records { ttl, .. }) => MailRoute::ImplicitMx {
            ttl: mx_ttl.min(ttl),
        },
        (NoData { ttl: v4 }, NoData { ttl: v6 }) => MailRoute::NoRoute {
            ttl: mx_ttl.min(v4).min(v6),
        },
        _ => MailRoute::Retry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeDns {
        mx: DnsAnswer<Vec<MxRecord>>,
        a: DnsAnswer<()>,
        aaaa: DnsAnswer<()>,
        calls: AtomicUsize,
    }

    impl FakeDns {
        fn new(mx: DnsAnswer<Vec<MxRecord>>, a: DnsAnswer<()>, aaaa: DnsAnswer<()>) -> Self {
            Self {
                mx,
                a,
                aaaa,
                calls: AtomicUsize::new(0),
            }
        }

        fn count(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    impl MailDnsResolver for FakeDns {
        fn mx<'a>(&'a self, _domain: &'a str) -> Lookup<'a, Vec<MxRecord>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { self.mx.clone() })
        }

        fn a<'a>(&'a self, _domain: &'a str) -> Lookup<'a, ()> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { self.a.clone() })
        }

        fn aaaa<'a>(&'a self, _domain: &'a str) -> Lookup<'a, ()> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { self.aaaa.clone() })
        }
    }

    fn mx(preference: u16, exchange: &str) -> MxRecord {
        MxRecord {
            preference,
            exchange: exchange.to_owned(),
        }
    }

    #[tokio::test]
    async fn explicit_mx_null_mx_and_errors_never_fall_back_to_a() {
        use DnsAnswer::*;
        let cases = [
            (
                Records {
                    value: vec![mx(10, "mail.example.com.")],
                    ttl: 90,
                },
                MailRoute::ExplicitMx { ttl: 90 },
            ),
            (
                Records {
                    value: vec![mx(0, ".")],
                    ttl: 60,
                },
                MailRoute::NullMx { ttl: 60 },
            ),
            (NxDomain { ttl: 20 }, MailRoute::DomainNotFound { ttl: 20 }),
            (TemporaryFailure, MailRoute::Retry),
            (
                Records {
                    value: vec![mx(0, "."), mx(10, "mail.example.com.")],
                    ttl: 60,
                },
                MailRoute::Retry,
            ),
            (
                Records {
                    value: vec![mx(10, ".")],
                    ttl: 60,
                },
                MailRoute::Retry,
            ),
            (
                Records {
                    value: vec![],
                    ttl: 60,
                },
                MailRoute::Retry,
            ),
        ];
        for (mx, expected) in cases {
            let dns = FakeDns::new(
                mx,
                Records {
                    value: (),
                    ttl: 100,
                },
                TemporaryFailure,
            );
            assert_eq!(check_mail_domain(&dns, "example.com").await, expected);
            assert_eq!(dns.count(), 1);
        }
    }

    #[tokio::test]
    async fn mx_nodata_allows_implicit_ipv4_or_ipv6_and_preserves_uncertainty() {
        use DnsAnswer::*;
        let cases = [
            (
                Records { value: (), ttl: 20 },
                NoData { ttl: 30 },
                MailRoute::ImplicitMx { ttl: 20 },
            ),
            (
                NoData { ttl: 30 },
                Records { value: (), ttl: 90 },
                MailRoute::ImplicitMx { ttl: 60 },
            ),
            (
                TemporaryFailure,
                Records { value: (), ttl: 40 },
                MailRoute::ImplicitMx { ttl: 40 },
            ),
            (
                NoData { ttl: 10 },
                NoData { ttl: 30 },
                MailRoute::NoRoute { ttl: 10 },
            ),
            (NoData { ttl: 10 }, TemporaryFailure, MailRoute::Retry),
            (NxDomain { ttl: 10 }, NoData { ttl: 30 }, MailRoute::Retry),
        ];
        for (a, aaaa, expected) in cases {
            let dns = FakeDns::new(NoData { ttl: 60 }, a, aaaa);
            assert_eq!(check_mail_domain(&dns, "example.com").await, expected);
            assert_eq!(dns.count(), 3);
        }
    }

    #[tokio::test]
    async fn invalid_domains_never_trigger_dns() {
        let dns = FakeDns::new(
            DnsAnswer::TemporaryFailure,
            DnsAnswer::TemporaryFailure,
            DnsAnswer::TemporaryFailure,
        );
        for invalid in ["a", "a@b.com", "a..com", "a.com.", "127.0.0.1", "a_b.com"] {
            assert_eq!(
                check_mail_domain(&dns, invalid).await,
                MailRoute::InvalidDomain
            );
        }
        assert_eq!(dns.count(), 0);
    }
}
