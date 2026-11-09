//! G-09 / #748: read-only inventory. A candidate is NEVER a delete permit.
use async_trait::async_trait;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryPrefix {
    ChapterVersions,
    ChapterAssets,
}

impl InventoryPrefix {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChapterVersions => "private/chapter-versions/",
            Self::ChapterAssets => "private/chapter-assets/",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryError {
    Invalid,
    Unavailable,
    LeaseLost,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryLease {
    pub prefix: InventoryPrefix,
    pub epoch: i64,
    pub cycle: i64,
    pub continuation_token: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryPage {
    pub keys: Vec<String>,
    pub next_token: Option<String>,
    pub truncated: bool,
}

impl InventoryPage {
    /// Refuse malformed responses; never checkpoint a page that may hide keys.
    pub fn validate(&self, lease: &InventoryLease) -> Result<(), InventoryError> {
        if self.keys.len() > 1000
            || self
                .keys
                .iter()
                .any(|key| !key.starts_with(lease.prefix.as_str()) || key.len() > 1024)
            || self.keys.iter().collect::<HashSet<_>>().len() != self.keys.len()
        {
            return Err(InventoryError::Invalid);
        }
        if self.truncated {
            if self.next_token.as_deref().is_none_or(str::is_empty)
                || self.next_token == lease.continuation_token
            {
                return Err(InventoryError::Invalid);
            }
        } else if self.next_token.is_some() {
            return Err(InventoryError::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InventoryOutcome {
    pub scanned: usize,
    pub unknown: usize,
    pub complete: bool,
}

#[async_trait]
pub trait InventoryLister: Send + Sync {
    async fn list_page(
        &self,
        prefix: InventoryPrefix,
        token: Option<&str>,
    ) -> Result<InventoryPage, InventoryError>;
}

#[async_trait]
pub trait InventoryCheckpoint: Send + Sync {
    async fn claim(
        &self,
        prefix: InventoryPrefix,
    ) -> Result<Option<InventoryLease>, InventoryError>;
    /// Commit candidates and continuation together, under a leased PG lock.
    async fn commit(
        &self,
        lease: &InventoryLease,
        page: &InventoryPage,
    ) -> Result<InventoryOutcome, InventoryError>;
}

/// At most one S3 LIST and one database transaction. No DELETE capability.
pub async fn scan_one<S, R>(
    storage: &S,
    registry: &R,
    prefix: InventoryPrefix,
) -> Result<Option<InventoryOutcome>, InventoryError>
where
    S: InventoryLister,
    R: InventoryCheckpoint,
{
    let Some(lease) = registry.claim(prefix).await? else {
        return Ok(None);
    };
    let page = storage
        .list_page(prefix, lease.continuation_token.as_deref())
        .await?;
    page.validate(&lease)?;
    registry.commit(&lease, &page).await.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease() -> InventoryLease {
        InventoryLease {
            prefix: InventoryPrefix::ChapterVersions,
            epoch: 1,
            cycle: 1,
            continuation_token: None,
        }
    }

    #[test]
    fn g09_inventory_rejects_unsafe_pages() {
        let mut p = InventoryPage {
            keys: vec!["private/chapter-versions/a.enc".into()],
            next_token: Some("opaque".into()),
            truncated: true,
        };
        assert_eq!(p.validate(&lease()), Ok(()));
        p.keys.push("private/chapter-versions/a.enc".into());
        assert_eq!(p.validate(&lease()), Err(InventoryError::Invalid));
        p.keys = vec!["private/chapter-assets/a.enc".into()];
        assert_eq!(p.validate(&lease()), Err(InventoryError::Invalid));
        p.keys = vec!["private/chapter-versions/a.enc".into()];
        p.next_token = None;
        assert_eq!(p.validate(&lease()), Err(InventoryError::Invalid));
        p.truncated = false;
        assert_eq!(p.validate(&lease()), Ok(()));
        p.next_token = Some("unexpected".into());
        assert_eq!(p.validate(&lease()), Err(InventoryError::Invalid));
        let mut later = lease();
        later.continuation_token = Some("opaque".into());
        p.truncated = true;
        p.next_token = Some("opaque".into());
        assert_eq!(p.validate(&later), Err(InventoryError::Invalid));
    }
}
