//! MAIL-01d: local, reviewed disposable-email data. No remote validation API.
use super::delivery_address;
use std::sync::OnceLock;

const BLOCKLIST: &str = include_str!("../../../../data/disposable_email_domains.txt");
const EXCEPTIONS: &str = include_str!("../../../../data/disposable_email_exceptions.txt");
const MAX_DOMAINS: usize = 50000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisposableStatus {
    Listed,
    NotListed,
    Exception,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DisposableMode {
    #[default]
    Observe,
    Enforce,
}

impl DisposableMode {
    pub fn should_block(self, status: DisposableStatus) -> bool {
        self == Self::Enforce && status == DisposableStatus::Listed
    }
}

struct Rules {
    blocked: Vec<&'static str>,
    exceptions: Vec<&'static str>,
}

static RULES: OnceLock<Result<Rules, &'static str>> = OnceLock::new();

/// Match the canonical transport domain, never the local part. A negative
/// match is not proof of an existing or permanent mailbox.
pub fn classify(address: &str) -> Result<DisposableStatus, &'static str> {
    let rules = RULES
        .get_or_init(|| Rules::parse(BLOCKLIST, EXCEPTIONS))
        .as_ref()
        .map_err(|error| *error)?;
    rules.classify(address)
}

impl Rules {
    fn parse(blocked: &'static str, exceptions: &'static str) -> Result<Self, &'static str> {
        let blocked = parse_list(blocked, false)?;
        if blocked.is_empty() {
            return Err("disposable email blocklist is empty");
        }
        Ok(Self {
            blocked,
            exceptions: parse_list(exceptions, true)?,
        })
    }

    fn classify(&self, address: &str) -> Result<DisposableStatus, &'static str> {
        let transport = delivery_address(address)?;
        let (_, domain) = transport
            .rsplit_once('@')
            .ok_or("invalid delivery address")?;
        let domain = domain.to_ascii_lowercase();
        let mut suffix = domain.as_str();
        // A reviewed exact exception overrides the list for that domain only.
        if self.exceptions.binary_search(&suffix).is_ok() {
            return Ok(DisposableStatus::Exception);
        }
        // Match only complete DNS labels, never arbitrary string suffixes.
        while let Some((_, parent)) = suffix.split_once('.') {
            if self.blocked.binary_search(&suffix).is_ok() {
                return Ok(DisposableStatus::Listed);
            }
            suffix = parent;
        }
        Ok(DisposableStatus::NotListed)
    }
}

fn parse_list(input: &'static str, comments: bool) -> Result<Vec<&'static str>, &'static str> {
    let mut domains = Vec::new();
    for domain in input.lines() {
        if comments && (domain.is_empty() || domain.starts_with('#')) {
            continue;
        }
        if domains.len() >= MAX_DOMAINS {
            return Err("disposable email domain list exceeds limit");
        }
        let candidate = format!("a@{domain}");
        if domain != domain.to_ascii_lowercase()
            || delivery_address(&candidate).ok().as_deref() != Some(candidate.as_str())
            || domains.last().is_some_and(|previous| *previous >= domain)
        {
            return Err("invalid or unsorted disposable email domain list");
        }
        domains.push(domain);
    }
    Ok(domains)
}

#[cfg(test)]
mod tests;
