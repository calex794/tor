//! E-owned types. A comment target is either a book or one exact chapter edition.
//! These types do not grant reading rights: E-02 must ask B/C/D before any write or read.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommentError {
    InvalidId,
    InvalidBody,
    InvalidAnchor,
    InvalidCommandKey,
    InvalidRelation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CommentId(i64);

impl CommentId {
    pub fn new(id: i64) -> Result<Self, CommentError> {
        (id > 0).then_some(Self(id)).ok_or(CommentError::InvalidId)
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetKind {
    Book,
    Chapter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommentTarget {
    kind: TargetKind,
    book_id: Option<i64>,
    chapter_id: Option<i64>,
    version_id: Option<i64>,
}

impl CommentTarget {
    pub fn book(book_id: i64) -> Result<Self, CommentError> {
        if book_id <= 0 {
            return Err(CommentError::InvalidId);
        }
        Ok(Self {
            kind: TargetKind::Book,
            book_id: Some(book_id),
            chapter_id: None,
            version_id: None,
        })
    }

    pub fn chapter(chapter_id: i64, version_id: i64) -> Result<Self, CommentError> {
        if chapter_id <= 0 || version_id <= 0 {
            return Err(CommentError::InvalidId);
        }
        Ok(Self {
            kind: TargetKind::Chapter,
            book_id: None,
            chapter_id: Some(chapter_id),
            version_id: Some(version_id),
        })
    }

    pub fn kind(self) -> TargetKind {
        self.kind
    }

    pub fn book_id(self) -> Option<i64> {
        self.book_id
    }

    pub fn chapter_version(self) -> Option<(i64, i64)> {
        self.chapter_id.zip(self.version_id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentBody(String);

impl CommentBody {
    pub const MAX_CHARS: usize = 2000;

    pub fn new(body: impl Into<String>) -> Result<Self, CommentError> {
        let body = body.into().trim().to_owned();
        if body.is_empty() || body.chars().count() > Self::MAX_CHARS || body.contains('\0') {
            return Err(CommentError::InvalidBody);
        }
        Ok(Self(body))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One 32-byte entropy key encoded as unpadded base64url (43 bytes).
/// A client cannot choose an actor, and reuse by the same actor must compare payloads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentIntentKey(String);

impl CommentIntentKey {
    pub fn new(key: impl Into<String>) -> Result<Self, CommentError> {
        let key = key.into();
        if key.len() != 43
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(CommentError::InvalidCommandKey);
        }
        Ok(Self(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Version-bound selection offsets use UTF-16 code units, as in reader progress.
/// A text quote is deliberately never stored in an anchor or an InboxEntry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentAnchor {
    pub chapter_version_id: i64,
    pub block_key: String,
    pub start_utf16: u32,
    pub end_utf16: u32,
}

impl CommentAnchor {
    pub fn new(
        chapter_version_id: i64,
        block_key: impl Into<String>,
        start_utf16: u32,
        end_utf16: u32,
    ) -> Result<Self, CommentError> {
        let block_key = block_key.into();
        if chapter_version_id <= 0
            || block_key.is_empty()
            || block_key.len() > 128
            || !block_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b':')
            || start_utf16 >= end_utf16
            || end_utf16 > 100_000
        {
            return Err(CommentError::InvalidAnchor);
        }
        Ok(Self {
            chapter_version_id,
            block_key,
            start_utf16,
            end_utf16,
        })
    }

    pub fn matches_target(&self, target: CommentTarget) -> bool {
        target
            .chapter_version()
            .is_some_and(|(_, version_id)| version_id == self.chapter_version_id)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CommentVisibility {
    #[default]
    Review,
    Visible,
    Hidden,
    Deleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comment {
    pub id: CommentId,
    pub author_user_id: i64,
    pub target: CommentTarget,
    pub body: CommentBody,
    pub anchor: Option<CommentAnchor>,
    pub visibility: CommentVisibility,
}

impl Comment {
    pub fn new(
        id: CommentId,
        author_user_id: i64,
        target: CommentTarget,
        body: CommentBody,
        anchor: Option<CommentAnchor>,
        visibility: CommentVisibility,
    ) -> Result<Self, CommentError> {
        if author_user_id <= 0 {
            return Err(CommentError::InvalidId);
        }
        if anchor.as_ref().is_some_and(|anchor| !anchor.matches_target(target)) {
            return Err(CommentError::InvalidAnchor);
        }
        Ok(Self {
            id,
            author_user_id,
            target,
            body,
            anchor,
            visibility,
        })
    }
}

/// Replies will refer to a single root comment; no second thread store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiscussionThread {
    pub root: CommentId,
}

/// Never put a body, selection quote or signed media link in an inbox record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InboxEntry {
    pub recipient_user_id: i64,
    pub source_comment_id: CommentId,
}

impl InboxEntry {
    pub fn new(recipient_user_id: i64, source_comment_id: CommentId) -> Result<Self, CommentError> {
        if recipient_user_id <= 0 {
            return Err(CommentError::InvalidId);
        }
        Ok(Self {
            recipient_user_id,
            source_comment_id,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelationKind {
    Following,
    Friend,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserRelation {
    pub actor_user_id: i64,
    pub subject_user_id: i64,
    pub kind: RelationKind,
}

impl UserRelation {
    pub fn new(actor_user_id: i64, subject_user_id: i64, kind: RelationKind) -> Result<Self, CommentError> {
        if actor_user_id <= 0 || subject_user_id <= 0 || actor_user_id == subject_user_id {
            return Err(CommentError::InvalidRelation);
        }
        Ok(Self {
            actor_user_id,
            subject_user_id,
            kind,
        })
    }
}

/// A scope descriptor is never a capability token or proof of an admin role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModeratorScope {
    Platform,
    Book(i64),
    Club(i64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e_01_id_and_body_limits() {
        assert_eq!(CommentId::new(0), Err(CommentError::InvalidId));
        assert!(CommentBody::new(" ").is_err());
        assert!(CommentBody::new("a".repeat(2001)).is_err());
        assert_eq!(CommentBody::new(" 😀 ").unwrap().as_str(), "😀");
        assert!(CommentBody::new("a\0b").is_err());
        assert!(CommentTarget::chapter(10, 0).is_err());
    }

    #[test]
    fn e_01_anchor_is_bound_to_immutable_edition_not_book() {
        let anchor = CommentAnchor::new(101, "p:3", 1, 3).unwrap();
        assert!(anchor.matches_target(CommentTarget::chapter(3, 101).unwrap()));
        assert!(!anchor.matches_target(CommentTarget::chapter(3, 102).unwrap()));
        assert!(!anchor.matches_target(CommentTarget::book(3).unwrap()));
        assert!(CommentAnchor::new(101, "p:3", 3, 3).is_err());
        assert!(Comment::new(
            CommentId::new(1).unwrap(),
            2,
            CommentTarget::book(3).unwrap(),
            CommentBody::new("Text").unwrap(),
            Some(anchor),
            CommentVisibility::Visible,
        )
        .is_err());
    }

    #[test]
    fn e_01_inbox_never_copies_private_text() {
        let entry = InboxEntry::new(8, CommentId::new(2).unwrap()).unwrap();
        assert_eq!(entry.source_comment_id.get(), 2);
        assert!(InboxEntry::new(0, CommentId::new(2).unwrap()).is_err());
        assert!(UserRelation::new(3, 3, RelationKind::Friend).is_err());
    }

    #[test]
    fn e_01_client_key_has_one_canonical_encoding() {
        assert!(CommentIntentKey::new("a".repeat(43)).is_ok());
        assert!(CommentIntentKey::new("a".repeat(42)).is_err());
        assert!(CommentIntentKey::new("a".repeat(42) + "=").is_err());
    }
}
