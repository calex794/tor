//! A current four-slot operational sample, never audience or an exhaustive URL inventory.
pub const PROBE_INTERVAL_SECONDS: u64 = 60;
pub const PROBE_FRESH_SECONDS: i64 = 180;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageProbeError {
    Invalid,
    Unavailable,
    Stale,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageSlot {
    Home,
    Catalog,
    Book,
    FreeChapter,
}
impl PageSlot {
    pub const ALL: [Self; 4] = [Self::Home, Self::Catalog, Self::Book, Self::FreeChapter];
    pub fn code(self) -> i16 {
        match self {
            Self::Home => 1,
            Self::Catalog => 2,
            Self::Book => 3,
            Self::FreeChapter => 4,
        }
    }
    pub fn parse(code: i16) -> Result<Self, PageProbeError> {
        Self::ALL
            .into_iter()
            .find(|slot| slot.code() == code)
            .ok_or(PageProbeError::Invalid)
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageTarget {
    pub slot: PageSlot,
    pub book_id: Option<i64>,
    pub chapter_id: Option<i64>,
    pub book_revision: Option<String>,
    pub chapter_revision: Option<String>,
    pub publication_revision: Option<String>,
    pub version_id: Option<i64>,
}
impl PageTarget {
    pub fn empty(slot: PageSlot) -> Self {
        Self {
            slot,
            book_id: None,
            chapter_id: None,
            book_revision: None,
            chapter_revision: None,
            publication_revision: None,
            version_id: None,
        }
    }
    pub fn path(&self) -> Option<String> {
        match self.slot {
            PageSlot::Home => Some("/".into()),
            PageSlot::Catalog => Some("/catalog".into()),
            PageSlot::Book => self.book_id.map(|id| format!("/books/{id}")),
            PageSlot::FreeChapter => self.chapter_id.map(|id| format!("/reader/{id}")),
        }
    }
}
#[derive(Clone, Debug)]
pub struct PageProbeLease {
    pub epoch: i64,
    pub origin: String,
    pub targets: Vec<PageTarget>,
}
#[derive(Clone, Debug)]
pub struct PageProbeResult {
    pub target: PageTarget,
    /// None means no verified HTTP response (network error or changed eligibility).
    pub status: Option<u16>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageState {
    Healthy,
    Lost,
    Unknown,
    NoTarget,
}
#[derive(Clone, Debug)]
pub struct PublicPageRow {
    pub slot: PageSlot,
    pub path: Option<String>,
    pub state: PageState,
    pub status: Option<u16>,
    pub last_success_at: Option<String>,
}
#[derive(Clone, Debug)]
pub struct PublicPageReport {
    pub measured_at: String,
    pub checked_at: Option<String>,
    pub age_seconds: Option<i64>,
    pub rows: Vec<PublicPageRow>,
}
impl PublicPageReport {
    pub fn checked_targets(&self) -> i32 {
        self.rows
            .iter()
            .filter(|row| matches!(row.state, PageState::Healthy | PageState::Lost))
            .count() as i32
    }
    pub fn expected_targets(&self) -> i32 {
        self.rows.iter().filter(|row| row.path.is_some()).count() as i32
    }
    pub fn lost_targets(&self) -> i64 {
        self.rows
            .iter()
            .filter(|row| row.state == PageState::Lost)
            .count() as i64
    }
}
