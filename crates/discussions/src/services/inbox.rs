//! E-03: bounded, actor-scoped Inbox; canonical comments and read markers only.
use crate::repositories::{
    inbox::{InboxEntry, PgInbox},
    public_book_comments::ListCommentsError,
};
use sqlx::PgPool;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InboxFilter {
    All,
    Unread,
    Read,
}

impl InboxFilter {
    pub fn parse(input: Option<&str>) -> Result<Self, ListCommentsError> {
        match input.unwrap_or("all") {
            "all" => Ok(Self::All),
            "unread" => Ok(Self::Unread),
            "read" => Ok(Self::Read),
            _ => Err(ListCommentsError::Invalid),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Unread => "unread",
            Self::Read => "read",
        }
    }

    fn read_predicate(self) -> Option<bool> {
        match self {
            Self::All => None,
            Self::Unread => Some(false),
            Self::Read => Some(true),
        }
    }
}

pub struct InboxPage {
    pub items: Vec<InboxEntry>,
    pub next_after: Option<i64>,
    pub filter: InboxFilter,
}

pub struct InboxService {
    repo: PgInbox,
}

impl InboxService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            repo: PgInbox::new(pool),
        }
    }

    pub async fn list(
        &self,
        actor_id: i64,
        filter: InboxFilter,
        after_id: Option<i64>,
        limit: u16,
    ) -> Result<InboxPage, ListCommentsError> {
        if limit == 0 || limit > 50 {
            return Err(ListCommentsError::Invalid);
        }
        let mut items = self
            .repo
            .received(actor_id, after_id, filter.read_predicate(), limit + 1)
            .await?;
        let has_more = items.len() > usize::from(limit);
        items.truncate(usize::from(limit));
        let next_after = has_more
            .then(|| items.last().map(|item| item.reply.comment_id))
            .flatten();
        Ok(InboxPage {
            items,
            next_after,
            filter,
        })
    }
}

/// The existing comment page supports an exclusive descending ID cursor.
pub fn comment_url(book_id: i64, comment_id: i64) -> String {
    let prefix = comment_id
        .checked_add(1)
        .map(|after| format!("?after={after}"))
        .unwrap_or_default();
    format!("/books/{book_id}/comments{prefix}#comment-{comment_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn e_03_inbox_filters_and_canonical_urls_are_bounded() {
        assert_eq!(InboxFilter::parse(None), Ok(InboxFilter::All));
        assert_eq!(InboxFilter::parse(Some("unread")), Ok(InboxFilter::Unread));
        assert_eq!(InboxFilter::parse(Some("read")), Ok(InboxFilter::Read));
        assert_eq!(InboxFilter::parse(Some("not-a-filter")), Err(ListCommentsError::Invalid));
        assert_eq!(comment_url(4, 26), "/books/4/comments?after=27#comment-26");
    }
}
