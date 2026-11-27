//! MAIL-01b2: DNS transport through the host's trusted recursive resolver.
//! No SMTP or third-party email-validation calls.
use crate::services::recipient_dns::{DnsAnswer, Lookup, MailDnsResolver, MxRecord};
use hickory_resolver::{
    config::{ResolveHosts, ResolverOpts},
    lookup::Lookup as HickoryLookup,
    net::{DnsError, NetError},
    proto::{
        op::ResponseCode,
        rr::{RData, RecordType},
    },
    TokioResolver,
};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct SystemMailDns {
    resolver: TokioResolver,
}

impl SystemMailDns {
    /// Use the operator's system DNS configuration. Fail closed at
    /// initialization; never silently switch to an external public resolver.
    #[cfg(any(unix, target_os = "windows"))]
    pub fn from_system() -> Result<Self, &'static str> {
        let mut builder =
            TokioResolver::builder_tokio().map_err(|_| "mail DNS configuration unavailable")?;
        configure(builder.options_mut());
        let resolver = builder
            .build()
            .map_err(|_| "mail DNS resolver unavailable")?;
        Ok(Self { resolver })
    }

    async fn lookup(
        &self,
        domain: &str,
        record_type: RecordType,
    ) -> Result<HickoryLookup, NetError> {
        // Absolute queries prevent expansion using resolver search suffixes.
        let fqdn = format!("{domain}.");
        self.resolver.lookup(fqdn, record_type).await
    }
}

fn configure(options: &mut ResolverOpts) {
    options.use_hosts_file = ResolveHosts::Never;
    options.timeout = Duration::from_millis(1500);
    options.attempts = 1;
    options.num_concurrent_reqs = 1;
    // Hickory 0.26.4 can reuse a cached CNAME target without retaining the
    // shorter alias TTL. Keep one cache at the application decision boundary.
    options.cache_size = 0;
    options.preserve_intermediates = true;
    options.positive_min_ttl = Some(Duration::ZERO);
    options.negative_min_ttl = Some(Duration::ZERO);
    options.positive_max_ttl = Some(Duration::from_secs(86400));
    options.negative_max_ttl = Some(Duration::from_secs(3600));
}

fn negative<T>(domain: &str, error: NetError) -> DnsAnswer<T> {
    // A recursive resolver's bare RCODE without SOA/negative TTL is not
    // enough evidence to reject an address permanently.
    if let NetError::Dns(DnsError::NoRecordsFound(records)) = error {
        // A CNAME target's negative answer does not prove the original
        // alias is absent. Do not cache it as a definitive original-domain
        // rejection without the complete chain and its expiry.
        let requested = format!("{domain}.");
        if !records
            .query
            .name
            .to_string()
            .eq_ignore_ascii_case(&requested)
        {
            return DnsAnswer::TemporaryFailure;
        }
        if records.soa.is_some() {
            if let Some(ttl) = records.negative_ttl {
                return match records.response_code {
                    ResponseCode::NXDomain => DnsAnswer::NxDomain { ttl },
                    ResponseCode::NoError => DnsAnswer::NoData { ttl },
                    _ => DnsAnswer::TemporaryFailure,
                };
            }
        }
    }
    DnsAnswer::TemporaryFailure
}

fn remaining_ttl(lookup: &HickoryLookup) -> u32 {
    let seconds = lookup
        .valid_until()
        .saturating_duration_since(Instant::now())
        .as_secs();
    let answer_ttl = lookup
        .answers()
        .iter()
        .map(|record| record.ttl)
        .min()
        .unwrap_or(0);
    u32::try_from(seconds).unwrap_or(u32::MAX).min(answer_ttl)
}

impl MailDnsResolver for SystemMailDns {
    fn mx<'a>(&'a self, domain: &'a str) -> Lookup<'a, Vec<MxRecord>> {
        Box::pin(async move {
            let answer = match self.lookup(domain, RecordType::MX).await {
                Ok(answer) => answer,
                Err(error) => return negative(domain, error),
            };
            let value: Vec<_> = answer
                .answers()
                .iter()
                .filter_map(|record| match &record.data {
                    RData::MX(mx) => Some(MxRecord {
                        preference: mx.preference,
                        exchange: mx.exchange.to_string(),
                    }),
                    _ => None,
                })
                .collect();
            if value.is_empty() {
                return DnsAnswer::TemporaryFailure;
            }
            DnsAnswer::Records {
                value,
                ttl: remaining_ttl(&answer),
            }
        })
    }

    fn a<'a>(&'a self, domain: &'a str) -> Lookup<'a, ()> {
        Box::pin(self.address(domain, RecordType::A))
    }

    fn aaaa<'a>(&'a self, domain: &'a str) -> Lookup<'a, ()> {
        Box::pin(self.address(domain, RecordType::AAAA))
    }
}

impl SystemMailDns {
    async fn address(&self, domain: &str, record_type: RecordType) -> DnsAnswer<()> {
        let answer = match self.lookup(domain, record_type).await {
            Ok(answer) => answer,
            Err(error) => return negative(domain, error),
        };
        if !answer
            .answers()
            .iter()
            .any(|record| record.record_type() == record_type)
        {
            return DnsAnswer::TemporaryFailure;
        }
        DnsAnswer::Records {
            value: (),
            ttl: remaining_ttl(&answer),
        }
    }
}

#[cfg(test)]
mod tests;
