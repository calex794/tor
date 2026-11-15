//! Anonymous card observations: no reader/session identity and no billing authority.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub const VIEWABLE_PERCENT: u16 = 60;
pub const VIEWABLE_DURATION_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExposureSurface {
    Home,
    Catalog,
    Search,
    BookDetail,
    ReaderEnd,
}
impl ExposureSurface {
    pub const ALL: [Self; 5] = [
        Self::Home,
        Self::Catalog,
        Self::Search,
        Self::BookDetail,
        Self::ReaderEnd,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Catalog => "catalog",
            Self::Search => "search",
            Self::BookDetail => "book_detail",
            Self::ReaderEnd => "reader_end",
        }
    }
}
impl FromStr for ExposureSurface {
    type Err = ();
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|v| v.as_str() == value)
            .ok_or(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromotionSlot {
    HomeFeatured,
    BookDetailRelated,
    ChapterEndFree,
}
impl PromotionSlot {
    pub const ALL: [Self; 3] = [
        Self::HomeFeatured,
        Self::BookDetailRelated,
        Self::ChapterEndFree,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HomeFeatured => "home_featured",
            Self::BookDetailRelated => "book_detail_related",
            Self::ChapterEndFree => "chapter_end_free",
        }
    }
    pub const fn surface(self) -> ExposureSurface {
        match self {
            Self::HomeFeatured => ExposureSurface::Home,
            Self::BookDetailRelated => ExposureSurface::BookDetail,
            Self::ChapterEndFree => ExposureSurface::ReaderEnd,
        }
    }
}
impl FromStr for PromotionSlot {
    type Err = ();
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|v| v.as_str() == value)
            .ok_or(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorialPlacement {
    pub campaign_id: i64,
    pub revision: i64,
    pub slot: PromotionSlot,
}

/// Trusted renderer input, never accepted as an HTTP event payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardObservation {
    pub book_id: i64,
    pub surface: ExposureSurface,
    pub editorial: Option<EditorialPlacement>,
}
impl CardObservation {
    pub fn valid(self) -> bool {
        self.book_id > 0
            && self.editorial.is_none_or(|p| {
                p.campaign_id > 0 && p.revision >= 0 && p.slot.surface() == self.surface
            })
    }
}

/// Enqueue an independent opaque proof without blocking page rendering.
/// Some means queued, not a persisted opportunity or a browser impression.
pub trait ObservationIssuer: Send + Sync {
    fn issue(&self, card: CardObservation) -> Option<String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationGuardError {
    Unavailable,
}

/// Revalidate editorial metadata at event time without coupling feature crates.
#[async_trait]
pub trait EditorialObservationGuard: Send + Sync {
    async fn allowed(&self, card: &CardObservation) -> Result<bool, ObservationGuardError>;

    /// Administrator report scope for every campaign state. The assigned book
    /// is immutable; the aggregate owner independently applies current visibility.
    /// An unavailable integration must not replace the scope with all books.
    async fn campaign_book(&self, _campaign_id: i64) -> Result<Option<i64>, ObservationGuardError> {
        Err(ObservationGuardError::Unavailable)
    }
}
