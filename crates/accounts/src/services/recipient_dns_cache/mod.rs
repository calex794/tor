//! MAIL-01c: bounded domain-level decisions and single-flight DNS lookups.
//! No full recipient addresses or external I/O while holding the index lock.
use super::recipient_dns::{check_mail_domain, public_domain, MailDnsResolver, MailRoute};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

const MAX_DOMAINS: usize = 2048;
const MAX_LOOKUPS: usize = 16;
const MAX_CALLERS: usize = 128;
const POSITIVE_CAP: u32 = 86400;
const NEGATIVE_CAP: u32 = 3600;
const RETRY_SECONDS: u32 = 5;
const SLOT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct MailDnsCache {
    resolver: Arc<dyn MailDnsResolver>,
    domains: Mutex<HashMap<String, Slot>>,
    callers: Semaphore,
    lookups: Semaphore,
    max_domains: usize,
}

type SharedSlot = Arc<AsyncMutex<Option<CachedRoute>>>;

struct Slot {
    state: SharedSlot,
    touched: Instant,
}

#[derive(Clone, Copy)]
struct CachedRoute {
    route: MailRoute,
    until: Instant,
}

impl CachedRoute {
    fn new(route: MailRoute, now: Instant) -> Option<Self> {
        let seconds = match route {
            MailRoute::ExplicitMx { ttl } | MailRoute::ImplicitMx { ttl } => ttl.min(POSITIVE_CAP),
            MailRoute::NullMx { ttl }
            | MailRoute::DomainNotFound { ttl }
            | MailRoute::NoRoute { ttl } => ttl.min(NEGATIVE_CAP),
            MailRoute::Retry => RETRY_SECONDS,
            MailRoute::InvalidDomain => return None,
        };
        if seconds == 0 {
            return None;
        }
        Some(Self {
            route,
            until: now + Duration::from_secs(u64::from(seconds)),
        })
    }

    fn current(self, now: Instant) -> Option<MailRoute> {
        if now >= self.until {
            return None;
        }
        // The entry lifetime was capped when created. Never return the
        // original TTL on a cache hit, which could extend downstream caches.
        let ttl = self.until.duration_since(now).as_secs() as u32;
        Some(match self.route {
            MailRoute::ExplicitMx { .. } => MailRoute::ExplicitMx { ttl },
            MailRoute::ImplicitMx { .. } => MailRoute::ImplicitMx { ttl },
            MailRoute::NullMx { .. } => MailRoute::NullMx { ttl },
            MailRoute::DomainNotFound { .. } => MailRoute::DomainNotFound { ttl },
            MailRoute::NoRoute { .. } => MailRoute::NoRoute { ttl },
            other => other,
        })
    }
}

impl MailDnsCache {
    pub fn new(resolver: Arc<dyn MailDnsResolver>) -> Self {
        Self::with_limits(resolver, MAX_DOMAINS, MAX_LOOKUPS, MAX_CALLERS)
    }

    fn with_limits(
        resolver: Arc<dyn MailDnsResolver>,
        max_domains: usize,
        max_lookups: usize,
        max_callers: usize,
    ) -> Self {
        Self {
            resolver,
            domains: Mutex::new(HashMap::new()),
            callers: Semaphore::new(max_callers),
            lookups: Semaphore::new(max_lookups),
            max_domains,
        }
    }

    fn slot(&self, domain: &str) -> Option<SharedSlot> {
        let mut domains = self.domains.lock().ok()?;
        let now = Instant::now();
        if let Some(slot) = domains.get_mut(domain) {
            slot.touched = now;
            return Some(Arc::clone(&slot.state));
        }
        if domains.len() >= self.max_domains {
            // Evict only an idle slot. Never detach an in-flight lookup or
            // its waiters; otherwise another request could start a duplicate.
            let key = domains
                .iter()
                .filter(|(_, slot)| Arc::strong_count(&slot.state) == 1)
                .min_by_key(|(_, slot)| slot.touched)
                .map(|(key, _)| key.clone())?;
            domains.remove(&key);
        }
        let state = Arc::new(AsyncMutex::new(None));
        domains.insert(
            domain.to_owned(),
            Slot {
                state: Arc::clone(&state),
                touched: now,
            },
        );
        Some(state)
    }

    /// Accept the canonical ASCII delivery domain (not a mailbox).
    /// Overload and cancellation are temporary, never proof of a bad address.
    pub async fn check(&self, domain: &str) -> MailRoute {
        if !public_domain(domain) {
            return MailRoute::InvalidDomain;
        }
        let domain = domain.to_ascii_lowercase();
        let Ok(_caller) = self.callers.try_acquire() else {
            return MailRoute::Retry;
        };
        let Some(slot) = self.slot(&domain) else {
            return MailRoute::Retry;
        };
        let Ok(mut state) = tokio::time::timeout(SLOT_TIMEOUT, slot.lock()).await else {
            return MailRoute::Retry;
        };
        if let Some(route) = state.and_then(|entry| entry.current(Instant::now())) {
            return route;
        }
        let Ok(_lookup) = self.lookups.try_acquire() else {
            return MailRoute::Retry;
        };
        // Holding this domain's asynchronous lock coalesces concurrent
        // callers. Cancellation releases the lock and both semaphore permits.
        let route = check_mail_domain(self.resolver.as_ref(), &domain).await;
        let now = Instant::now();
        *state = CachedRoute::new(route, now);
        state.and_then(|entry| entry.current(now)).unwrap_or(route)
    }
}

#[cfg(test)]
mod tests;
