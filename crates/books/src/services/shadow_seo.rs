//! Shadow's per-book SEO policy. A book may be readable on both domains,
//! but only one domain is preferred for indexing the same book pages.
//! This policy does not grant reading access to a chapter.
use crate::domain::Visibility;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoIndexOwner {
    Pandaolate,
    Shadow,
}

impl SeoIndexOwner {
    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "pandaolate" => Some(Self::Pandaolate),
            "shadow" => Some(Self::Shadow),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShadowBookSeo {
    pub visibility: Visibility,
    pub admin_hold: bool,
    pub is_deleted: bool,
    pub shadow_enabled: bool,
    pub seo_index_owner: SeoIndexOwner,
}

impl ShadowBookSeo {
    pub fn public_book(self) -> bool {
        self.visibility == Visibility::Public && !self.admin_hold && !self.is_deleted
    }

    /// Manual enablement, not the price of any chapter, decides Shadow listing.
    pub fn listed_on_shadow(self) -> bool {
        self.public_book() && self.shadow_enabled
    }

    /// An inconsistent disabled Shadow flag can never cause Pandaolate to
    /// lose indexing. Changing a live book's owner requires an explicit
    /// editorial/SEO decision, not an implicit opt-in side effect.
    pub fn index_owner(self) -> Option<SeoIndexOwner> {
        if !self.public_book() {
            return None;
        }
        if self.shadow_enabled && self.seo_index_owner == SeoIndexOwner::Shadow {
            Some(SeoIndexOwner::Shadow)
        } else {
            Some(SeoIndexOwner::Pandaolate)
        }
    }

    pub fn indexable_on(self, site: SeoIndexOwner) -> bool {
        self.index_owner() == Some(site)
    }

    /// HTTP 404/410 for withdrawn content is enforced by the reader.
    /// Robots metadata is separate from authorization and access control.
    pub fn robots_meta(self, site: SeoIndexOwner) -> &'static str {
        if self.indexable_on(site) {
            "index,follow"
        } else {
            "noindex,follow"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SeoIndexOwner, ShadowBookSeo};
    use crate::domain::Visibility;

    fn default_book() -> ShadowBookSeo {
        ShadowBookSeo {
            visibility: Visibility::Public,
            admin_hold: false,
            is_deleted: false,
            shadow_enabled: false,
            seo_index_owner: SeoIndexOwner::Pandaolate,
        }
    }

    #[test]
    fn shadow_is_opt_in_and_seo_never_switches_implicitly() {
        let mut book = default_book();
        assert!(!book.listed_on_shadow());
        assert_eq!(book.index_owner(), Some(SeoIndexOwner::Pandaolate));
        assert_eq!(
            book.robots_meta(SeoIndexOwner::Shadow),
            "noindex,follow"
        );

        book.shadow_enabled = true;
        assert!(book.listed_on_shadow());
        assert_eq!(book.index_owner(), Some(SeoIndexOwner::Pandaolate));

        // SEO ownership is a separate, explicit editorial decision.
        book.seo_index_owner = SeoIndexOwner::Shadow;
        assert_eq!(book.index_owner(), Some(SeoIndexOwner::Shadow));
        assert!(!book.indexable_on(SeoIndexOwner::Pandaolate));

        // Defensive fallback; PostgreSQL also rejects this invalid pairing.
        book.shadow_enabled = false;
        assert!(!book.listed_on_shadow());
        assert_eq!(book.index_owner(), Some(SeoIndexOwner::Pandaolate));
    }

    #[test]
    fn draft_held_and_deleted_books_are_never_indexed_or_listed() {
        let mut book = default_book();
        book.shadow_enabled = true;
        book.seo_index_owner = SeoIndexOwner::Shadow;

        for state in [Visibility::Draft, Visibility::Public] {
            book.visibility = state;
            book.admin_hold = state == Visibility::Public;
            assert!(!book.listed_on_shadow());
            assert_eq!(book.index_owner(), None);
        }
        book.visibility = Visibility::Public;
        book.admin_hold = false;
        book.is_deleted = true;
        assert!(!book.listed_on_shadow());
        assert_eq!(book.index_owner(), None);
    }

    #[test]
    fn unknown_database_seo_owner_is_rejected() {
        assert_eq!(
            SeoIndexOwner::from_db("pandaolate"),
            Some(SeoIndexOwner::Pandaolate)
        );
        assert_eq!(
            SeoIndexOwner::from_db("shadow"),
            Some(SeoIndexOwner::Shadow)
        );
        assert_eq!(SeoIndexOwner::from_db("SHADOW"), None);
        assert_eq!(SeoIndexOwner::from_db(""), None);
    }
}
