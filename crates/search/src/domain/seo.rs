//! SEO decisions use facts already authorized by the owning book/reader domain.
//! Never treat this model as permission to read a book or a chapter.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoSite {
    Pandaolate,
    Shadow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoMode {
    Disabled,
    Pilot,
    Public,
}

/// AnonymousPublic means that B/C already verified the public access contract.
/// Raw database visibility alone must not be promoted to this state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoVisibility {
    AnonymousPublic,
    Draft,
    Held,
    Deleted,
    Scheduled,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoChapterAccess {
    Free,
    Paid,
    Private,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeoBookFacts {
    pub book_id: u64,
    pub visibility: SeoVisibility,
    /// True only when an anonymous reader has at least one published free chapter.
    pub has_public_content: bool,
    pub index_owner: SeoSite,
    pub shadow_enabled: bool,
    pub pilot_approved: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeoChapterFacts {
    pub chapter_id: u64,
    pub book: SeoBookFacts,
    pub visibility: SeoVisibility,
    pub access: SeoChapterAccess,
    pub has_public_version: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoCatalogView {
    Canonical,
    Filtered,
    Paginated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoBookView {
    Canonical,
    PaginatedToc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoPage {
    Home,
    Catalog(SeoCatalogView),
    Book {
        book: SeoBookFacts,
        view: SeoBookView,
    },
    Chapter(SeoChapterFacts),
    Private,
    Api,
    Service,
    Asset,
    SeoArtifact,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoRobots {
    IndexFollow,
    NoIndexFollow,
    NoIndexNoFollow,
}

impl SeoRobots {
    pub fn meta(self) -> &'static str {
        match self {
            Self::IndexFollow => "index,follow",
            Self::NoIndexFollow => "noindex,follow",
            Self::NoIndexNoFollow => "noindex,nofollow",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoReason {
    Indexable,
    Disabled,
    PilotNotApproved,
    InvalidIdentity,
    BookNotPublic,
    BookHasNoPublicContent,
    ChapterNotPublic,
    ChapterNotFree,
    ChapterHasNoPublicVersion,
    ShadowDisabled,
    DifferentIndexOwner,
    DuplicateView,
    ExcludedRoute,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeoDecision {
    pub robots: SeoRobots,
    pub sitemap_included: bool,
    /// Relative path only. HTTP must join this with a trusted configured origin.
    pub canonical_path: Option<String>,
    pub reason: SeoReason,
}

impl SeoDecision {
    pub fn indexable(&self) -> bool {
        self.robots == SeoRobots::IndexFollow
    }
}

/// Only explicitly registered public HTML routes are candidates for indexing.
/// This is not the Route Intelligence classifier and does not import RIL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoRouteClass {
    Home,
    Catalog,
    Book,
    Chapter,
    Excluded,
}

pub fn seo_route_class(method: &str, template: &str) -> SeoRouteClass {
    match (method, template) {
        ("GET", "/") => SeoRouteClass::Home,
        ("GET", "/catalog") => SeoRouteClass::Catalog,
        ("GET", "/books/{id}") => SeoRouteClass::Book,
        ("GET", "/reader/{id}") => SeoRouteClass::Chapter,
        _ => SeoRouteClass::Excluded,
    }
}
