//! Pure SEO policy. Rendering, SQL and access control belong to other layers.

use crate::domain::seo::SeoChapterAccess;
use crate::domain::seo::{
    SeoBookFacts, SeoBookView, SeoCatalogView, SeoDecision, SeoMode, SeoPage, SeoReason, SeoRobots,
    SeoSite, SeoVisibility,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeoPolicy {
    pub site: SeoSite,
    pub mode: SeoMode,
}

impl SeoPolicy {
    pub fn new(site: SeoSite, mode: SeoMode) -> Self {
        Self { site, mode }
    }

    pub fn decide(self, page: SeoPage) -> SeoDecision {
        match page {
            SeoPage::Home => self.sitewide("/"),
            SeoPage::Catalog(SeoCatalogView::Canonical) => self.sitewide("/catalog"),
            SeoPage::Catalog(SeoCatalogView::Filtered | SeoCatalogView::Paginated) => {
                denied(SeoReason::DuplicateView)
            }
            SeoPage::Book { book, view } => {
                if view != SeoBookView::Canonical {
                    return denied(SeoReason::DuplicateView);
                }
                match self.book_gate(book) {
                    Ok(()) => indexable(format!("/books/{}", book.book_id)),
                    Err(reason) => denied(reason),
                }
            }
            SeoPage::Chapter(chapter) => {
                if chapter.chapter_id == 0 {
                    return denied(SeoReason::InvalidIdentity);
                }
                if chapter.visibility != SeoVisibility::AnonymousPublic {
                    return denied(SeoReason::ChapterNotPublic);
                }
                if chapter.access != SeoChapterAccess::Free {
                    return denied(SeoReason::ChapterNotFree);
                }
                if !chapter.has_public_version {
                    return denied(SeoReason::ChapterHasNoPublicVersion);
                }
                match self.book_gate(chapter.book) {
                    Ok(()) => indexable(format!("/reader/{}", chapter.chapter_id)),
                    Err(reason) => denied(reason),
                }
            }
            SeoPage::Private
            | SeoPage::Api
            | SeoPage::Service
            | SeoPage::Asset
            | SeoPage::SeoArtifact
            | SeoPage::Unknown => denied(SeoReason::ExcludedRoute),
        }
    }

    fn sitewide(self, path: &str) -> SeoDecision {
        match self.mode {
            SeoMode::Public => indexable(path.to_owned()),
            SeoMode::Pilot => denied(SeoReason::PilotNotApproved),
            SeoMode::Disabled => denied(SeoReason::Disabled),
        }
    }

    fn book_gate(self, book: SeoBookFacts) -> Result<(), SeoReason> {
        if book.book_id == 0 {
            return Err(SeoReason::InvalidIdentity);
        }
        if book.visibility != SeoVisibility::AnonymousPublic {
            return Err(SeoReason::BookNotPublic);
        }
        if !book.has_public_content {
            return Err(SeoReason::BookHasNoPublicContent);
        }
        if self.site == SeoSite::Shadow && !book.shadow_enabled {
            return Err(SeoReason::ShadowDisabled);
        }
        if book.index_owner != self.site {
            return Err(SeoReason::DifferentIndexOwner);
        }
        match self.mode {
            SeoMode::Disabled => Err(SeoReason::Disabled),
            SeoMode::Pilot if !book.pilot_approved => Err(SeoReason::PilotNotApproved),
            SeoMode::Pilot | SeoMode::Public => Ok(()),
        }
    }
}

fn indexable(path: String) -> SeoDecision {
    SeoDecision {
        robots: SeoRobots::IndexFollow,
        sitemap_included: true,
        canonical_path: Some(path),
        reason: SeoReason::Indexable,
    }
}

fn denied(reason: SeoReason) -> SeoDecision {
    // Pages readable on another domain, temporary opt-outs and duplicate views
    // may still contain useful ordinary links. Restricted material must not.
    let robots = match reason {
        SeoReason::Disabled
        | SeoReason::PilotNotApproved
        | SeoReason::DifferentIndexOwner
        | SeoReason::DuplicateView => SeoRobots::NoIndexFollow,
        _ => SeoRobots::NoIndexNoFollow,
    };
    SeoDecision {
        robots,
        sitemap_included: false,
        canonical_path: None,
        reason,
    }
}
