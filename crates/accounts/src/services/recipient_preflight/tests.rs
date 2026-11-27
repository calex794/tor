use super::*;
use crate::services::recipient_dns::{DnsAnswer, Lookup, MxRecord};
use std::sync::Mutex;

struct FakeDns {
    mx: DnsAnswer<Vec<MxRecord>>,
    a: DnsAnswer<()>,
    aaaa: DnsAnswer<()>,
    queries: Mutex<Vec<String>>,
}

impl FakeDns {
    fn new(mx: DnsAnswer<Vec<MxRecord>>, a: DnsAnswer<()>, aaaa: DnsAnswer<()>) -> Arc<Self> {
        Arc::new(Self {
            mx,
            a,
            aaaa,
            queries: Mutex::new(Vec::new()),
        })
    }

    fn queried(&self, kind: &str, domain: &str) {
        self.queries
            .lock()
            .unwrap()
            .push(format!("{kind}:{domain}"));
    }

    fn queries(&self) -> Vec<String> {
        self.queries.lock().unwrap().clone()
    }
}

impl MailDnsResolver for FakeDns {
    fn mx<'a>(&'a self, domain: &'a str) -> Lookup<'a, Vec<MxRecord>> {
        self.queried("MX", domain);
        Box::pin(async { self.mx.clone() })
    }

    fn a<'a>(&'a self, domain: &'a str) -> Lookup<'a, ()> {
        self.queried("A", domain);
        Box::pin(async { self.a.clone() })
    }

    fn aaaa<'a>(&'a self, domain: &'a str) -> Lookup<'a, ()> {
        self.queried("AAAA", domain);
        Box::pin(async { self.aaaa.clone() })
    }
}

fn mx(exchange: &str, preference: u16) -> DnsAnswer<Vec<MxRecord>> {
    DnsAnswer::Records {
        value: vec![MxRecord {
            preference,
            exchange: exchange.to_owned(),
        }],
        ttl: 60,
    }
}

fn explicit() -> Arc<FakeDns> {
    FakeDns::new(
        mx("mx.example.com.", 10),
        DnsAnswer::TemporaryFailure,
        DnsAnswer::TemporaryFailure,
    )
}

#[tokio::test]
async fn invalid_and_enforced_disposable_recipients_do_not_query_dns() {
    let dns = explicit();
    let preflight = RecipientPreflight::new(dns.clone(), DisposableMode::Enforce);
    for address in ["invalid", "a@@gmail.com", "a@gmail.com\n"] {
        assert!(matches!(
            preflight.check(address).await,
            RecipientDecision::Reject(RecipientRejection::InvalidAddress)
        ));
    }
    assert!(matches!(
        preflight.check("a@sub.mailinator.com").await,
        RecipientDecision::Reject(RecipientRejection::DisposableDomain)
    ));
    assert!(dns.queries().is_empty());
}

#[tokio::test]
async fn observe_does_not_block_and_typo_hints_never_rewrite_addresses() {
    let dns = explicit();
    let preflight = RecipientPreflight::new(dns.clone(), DisposableMode::default());
    let RecipientDecision::Ready(listed) = preflight.check("a@mailinator.com").await else {
        panic!("Observe must not block a routable listed domain");
    };
    assert_eq!(listed.disposable_status(), DisposableStatus::Listed);
    let RecipientDecision::Ready(typo) = preflight.check("a@gmial.com").await else {
        panic!("a typo hint is not a rejection");
    };
    assert_eq!(typo.transport_address(), "a@gmial.com");
    assert_eq!(dns.queries(), ["MX:mailinator.com", "MX:gmial.com"]);
}

#[tokio::test]
async fn idna_aliases_share_domain_cache_without_changing_local_parts() {
    let dns = explicit();
    let preflight = RecipientPreflight::new(dns.clone(), DisposableMode::default());
    for (input, expected) in [
        ("A+Books@пример.рф", "A+Books@xn--e1afmkfd.xn--p1ai"),
        (
            "second@xn--e1afmkfd.xn--p1ai",
            "second@xn--e1afmkfd.xn--p1ai",
        ),
    ] {
        let RecipientDecision::Ready(recipient) = preflight.check(input).await else {
            panic!("valid IDNA transport address rejected");
        };
        assert_eq!(recipient.transport_address(), expected);
    }
    assert_eq!(dns.queries(), ["MX:xn--e1afmkfd.xn--p1ai"]);
}

#[tokio::test]
async fn definitive_negative_routes_reject_only_the_current_attempt() {
    use DnsAnswer::*;
    for (mx, a, aaaa, reason, calls) in [
        (
            mx(".", 0),
            TemporaryFailure,
            TemporaryFailure,
            RecipientRejection::NullMx,
            1,
        ),
        (
            NxDomain { ttl: 60 },
            TemporaryFailure,
            TemporaryFailure,
            RecipientRejection::DomainNotFound,
            1,
        ),
        (
            NoData { ttl: 60 },
            NoData { ttl: 60 },
            NoData { ttl: 60 },
            RecipientRejection::NoRoute,
            3,
        ),
    ] {
        let dns = FakeDns::new(mx, a, aaaa);
        let preflight = RecipientPreflight::new(dns.clone(), DisposableMode::default());
        assert!(matches!(
            preflight.check("a@gmail.com").await,
            RecipientDecision::Reject(actual) if actual == reason
        ));
        assert_eq!(dns.queries().len(), calls);
    }
}

#[tokio::test]
async fn implicit_routes_are_allowed_but_uncertainty_defers() {
    use DnsAnswer::*;
    let dns = FakeDns::new(
        NoData { ttl: 60 },
        Records { value: (), ttl: 30 },
        TemporaryFailure,
    );
    let preflight = RecipientPreflight::new(dns.clone(), DisposableMode::default());
    assert!(matches!(
        preflight.check("a@gmail.com").await,
        RecipientDecision::Ready(_)
    ));
    assert_eq!(dns.queries().len(), 3);

    for answer in [
        TemporaryFailure,
        Records {
            value: vec![],
            ttl: 60,
        },
    ] {
        let dns = FakeDns::new(answer, NoData { ttl: 60 }, NoData { ttl: 60 });
        let preflight = RecipientPreflight::new(dns.clone(), DisposableMode::default());
        assert!(matches!(
            preflight.check("a@gmail.com").await,
            RecipientDecision::Defer(RecipientDeferral::DnsUnavailable)
        ));
        assert_eq!(dns.queries().len(), 1);
    }
}
