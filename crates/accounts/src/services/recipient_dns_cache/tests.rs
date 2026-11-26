use super::*;
use crate::services::recipient_dns::{DnsAnswer, Lookup, MxRecord};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingDns {
    calls: AtomicUsize,
    block: bool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl CountingDns {
    fn new(block: bool) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            block,
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        }
    }

    fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl MailDnsResolver for CountingDns {
    fn mx<'a>(&'a self, domain: &'a str) -> Lookup<'a, Vec<MxRecord>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.block {
                self.entered.notify_one();
                self.release.notified().await;
            }
            match domain {
                "missing.example" => DnsAnswer::NxDomain { ttl: 7200 },
                "retry.example" => DnsAnswer::TemporaryFailure,
                "zero.example" => DnsAnswer::Records {
                    value: vec![MxRecord {
                        preference: 10,
                        exchange: "mail.example.".to_owned(),
                    }],
                    ttl: 0,
                },
                _ => DnsAnswer::Records {
                    value: vec![MxRecord {
                        preference: 10,
                        exchange: "mail.example.".to_owned(),
                    }],
                    ttl: 172800,
                },
            }
        })
    }

    fn a<'a>(&'a self, _domain: &'a str) -> Lookup<'a, ()> {
        panic!("explicit MX must not query A")
    }

    fn aaaa<'a>(&'a self, _domain: &'a str) -> Lookup<'a, ()> {
        panic!("explicit MX must not query AAAA")
    }
}

#[test]
fn ttl_caps_expiry_and_zero_ttl_are_respected() {
    let now = Instant::now();
    let routes = [
        (MailRoute::ExplicitMx { ttl: 172800 }, 86400),
        (MailRoute::ImplicitMx { ttl: 60 }, 60),
        (MailRoute::NullMx { ttl: 7200 }, 3600),
        (MailRoute::DomainNotFound { ttl: 20 }, 20),
        (MailRoute::NoRoute { ttl: 7200 }, 3600),
        (MailRoute::Retry, 5),
    ];
    for (route, expected) in routes {
        let entry = CachedRoute::new(route, now).unwrap();
        assert_eq!(entry.until, now + Duration::from_secs(expected));
        assert!(entry.current(now + Duration::from_secs(expected - 1)).is_some());
        assert_eq!(entry.current(entry.until), None);
    }
    let entry = CachedRoute::new(MailRoute::ExplicitMx { ttl: 120 }, now).unwrap();
    assert_eq!(
        entry.current(now + Duration::from_secs(40)),
        Some(MailRoute::ExplicitMx { ttl: 80 })
    );
    assert!(CachedRoute::new(MailRoute::NoRoute { ttl: 0 }, now).is_none());
    assert!(CachedRoute::new(MailRoute::InvalidDomain, now).is_none());
}

#[tokio::test]
async fn case_insensitive_cache_negative_retry_and_zero_ttl() {
    let dns = Arc::new(CountingDns::new(false));
    let cache = MailDnsCache::new(dns.clone());
    assert_eq!(
        cache.check("Reader@Example.com").await,
        MailRoute::InvalidDomain
    );
    assert_eq!(dns.count(), 0);
    for domain in ["EXAMPLE.COM", "example.com", "Example.Com"] {
        assert!(matches!(
            cache.check(domain).await,
            MailRoute::ExplicitMx { ttl: 1..=86400 }
        ));
    }
    for _ in 0..2 {
        assert!(matches!(
            cache.check("missing.example").await,
            MailRoute::DomainNotFound { ttl: 1..=3600 }
        ));
        assert_eq!(cache.check("retry.example").await, MailRoute::Retry);
        assert_eq!(
            cache.check("zero.example").await,
            MailRoute::ExplicitMx { ttl: 0 }
        );
    }
    assert_eq!(dns.count(), 5);
}

#[tokio::test]
async fn bounded_index_and_busy_slot_are_not_evicted() {
    let dns = Arc::new(CountingDns::new(true));
    let cache = Arc::new(MailDnsCache::with_limits(dns.clone(), 1, 1, 8));
    let leader = {
        let cache = Arc::clone(&cache);
        tokio::spawn(async move { cache.check("example.com").await })
    };
    dns.entered.notified().await;
    let follower = {
        let cache = Arc::clone(&cache);
        tokio::spawn(async move { cache.check("EXAMPLE.COM").await })
    };
    assert_eq!(cache.check("other.example").await, MailRoute::Retry);
    assert_eq!(cache.domains.lock().unwrap().len(), 1);
    dns.release.notify_one();
    for task in [leader, follower] {
        assert!(matches!(
            task.await.unwrap(),
            MailRoute::ExplicitMx { .. }
        ));
    }
    assert_eq!(dns.count(), 1);
    let second = {
        let cache = Arc::clone(&cache);
        tokio::spawn(async move { cache.check("other.example").await })
    };
    dns.entered.notified().await;
    dns.release.notify_one();
    assert!(matches!(
        second.await.unwrap(),
        MailRoute::ExplicitMx { .. }
    ));
    assert_eq!(dns.count(), 2);
    let domains = cache.domains.lock().unwrap();
    assert_eq!(domains.len(), 1);
    assert!(domains.contains_key("other.example"));
}

#[tokio::test]
async fn global_lookup_limit_and_cancelled_leader_release_capacity() {
    let dns = Arc::new(CountingDns::new(true));
    let cache = Arc::new(MailDnsCache::with_limits(dns.clone(), 4, 1, 8));
    let first = {
        let cache = Arc::clone(&cache);
        tokio::spawn(async move { cache.check("example.com").await })
    };
    dns.entered.notified().await;
    assert_eq!(cache.check("other.example").await, MailRoute::Retry);
    assert_eq!(dns.count(), 1);
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let second = {
        let cache = Arc::clone(&cache);
        tokio::spawn(async move { cache.check("other.example").await })
    };
    dns.entered.notified().await;
    dns.release.notify_one();
    assert!(matches!(
        second.await.unwrap(),
        MailRoute::ExplicitMx { .. }
    ));
    assert_eq!(dns.count(), 2);
}
