use tor_search::domain::seo::{
    seo_route_class, SeoBookFacts, SeoBookView, SeoCatalogView, SeoChapterAccess, SeoChapterFacts,
    SeoMode, SeoPage, SeoReason, SeoRobots, SeoRouteClass, SeoSite, SeoVisibility,
};
use tor_search::services::seo::SeoPolicy;

fn book() -> SeoBookFacts {
    SeoBookFacts {
        book_id: 42,
        visibility: SeoVisibility::AnonymousPublic,
        has_public_content: true,
        index_owner: SeoSite::Pandaolate,
        shadow_enabled: false,
        pilot_approved: false,
    }
}

fn chapter() -> SeoChapterFacts {
    SeoChapterFacts {
        chapter_id: 123,
        book: book(),
        visibility: SeoVisibility::AnonymousPublic,
        access: SeoChapterAccess::Free,
        has_public_version: true,
    }
}

fn policy(site: SeoSite, mode: SeoMode) -> SeoPolicy {
    SeoPolicy::new(site, mode)
}

#[test]
fn f_08_seo_01_indexing_is_disabled_by_default() {
    let off = policy(SeoSite::Pandaolate, SeoMode::Disabled);
    for page in [
        SeoPage::Home,
        SeoPage::Catalog(SeoCatalogView::Canonical),
        SeoPage::Book {
            book: book(),
            view: SeoBookView::Canonical,
        },
        SeoPage::Chapter(chapter()),
    ] {
        let decision = off.decide(page);
        assert_eq!(decision.reason, SeoReason::Disabled);
        assert_eq!(decision.robots, SeoRobots::NoIndexFollow);
        assert!(!decision.indexable());
        assert!(!decision.sitemap_included);
        assert!(decision.canonical_path.is_none());
    }
}

#[test]
fn f_08_seo_01_public_pages_have_exactly_one_canonical_path() {
    let open = policy(SeoSite::Pandaolate, SeoMode::Public);
    for (page, path) in [
        (SeoPage::Home, "/"),
        (SeoPage::Catalog(SeoCatalogView::Canonical), "/catalog"),
        (
            SeoPage::Book {
                book: book(),
                view: SeoBookView::Canonical,
            },
            "/books/42",
        ),
        (SeoPage::Chapter(chapter()), "/reader/123"),
    ] {
        let decision = open.decide(page);
        assert!(decision.indexable());
        assert_eq!(decision.robots.meta(), "index,follow");
        assert!(decision.sitemap_included);
        assert_eq!(decision.canonical_path.as_deref(), Some(path));
        assert_eq!(decision.reason, SeoReason::Indexable);
    }
}

#[test]
fn f_08_seo_01_noncanonical_and_unregistered_routes_never_index() {
    let open = policy(SeoSite::Pandaolate, SeoMode::Public);
    let cases = [
        SeoPage::Catalog(SeoCatalogView::Filtered),
        SeoPage::Catalog(SeoCatalogView::Paginated),
        SeoPage::Book {
            book: book(),
            view: SeoBookView::PaginatedToc,
        },
        SeoPage::Private,
        SeoPage::Api,
        SeoPage::Service,
        SeoPage::Asset,
        SeoPage::SeoArtifact,
        SeoPage::Unknown,
    ];
    for page in cases {
        let decision = open.decide(page);
        assert!(!decision.indexable());
        assert!(!decision.sitemap_included);
        assert!(decision.canonical_path.is_none());
    }
    for (method, template, expected) in [
        ("GET", "/", SeoRouteClass::Home),
        ("GET", "/catalog", SeoRouteClass::Catalog),
        ("GET", "/books/{id}", SeoRouteClass::Book),
        ("GET", "/reader/{id}", SeoRouteClass::Chapter),
        ("POST", "/books/{id}", SeoRouteClass::Excluded),
        ("GET", "/me", SeoRouteClass::Excluded),
        ("GET", "/ops/overview", SeoRouteClass::Excluded),
        ("GET", "/api/v1/books/{id}/toc", SeoRouteClass::Excluded),
        ("GET", "/sitemap.xml", SeoRouteClass::Excluded),
        ("GET", "/robots.txt", SeoRouteClass::Excluded),
        ("GET", "/books/42", SeoRouteClass::Excluded),
        ("GET", "/unknown", SeoRouteClass::Excluded),
    ] {
        assert_eq!(seo_route_class(method, template), expected);
    }
}

#[test]
fn f_08_seo_01_book_visibility_and_content_are_strict() {
    let open = policy(SeoSite::Pandaolate, SeoMode::Public);
    for visibility in [
        SeoVisibility::Draft,
        SeoVisibility::Held,
        SeoVisibility::Deleted,
        SeoVisibility::Scheduled,
        SeoVisibility::Unknown,
    ] {
        let facts = SeoBookFacts {
            visibility,
            ..book()
        };
        let decision = open.decide(SeoPage::Book {
            book: facts,
            view: SeoBookView::Canonical,
        });
        assert_eq!(decision.reason, SeoReason::BookNotPublic);
        assert_eq!(decision.robots.meta(), "noindex,nofollow");
        assert!(!decision.sitemap_included);
    }
    for (facts, reason) in [
        (
            SeoBookFacts {
                book_id: 0,
                ..book()
            },
            SeoReason::InvalidIdentity,
        ),
        (
            SeoBookFacts {
                has_public_content: false,
                ..book()
            },
            SeoReason::BookHasNoPublicContent,
        ),
    ] {
        assert_eq!(
            open.decide(SeoPage::Book {
                book: facts,
                view: SeoBookView::Canonical,
            })
            .reason,
            reason
        );
    }
}

#[test]
fn f_08_seo_01_paid_scheduled_unpublished_chapters_are_excluded() {
    let open = policy(SeoSite::Pandaolate, SeoMode::Public);
    for access in [
        SeoChapterAccess::Paid,
        SeoChapterAccess::Private,
        SeoChapterAccess::Unknown,
    ] {
        assert_eq!(
            open.decide(SeoPage::Chapter(SeoChapterFacts {
                access,
                ..chapter()
            }))
            .reason,
            SeoReason::ChapterNotFree
        );
    }
    for visibility in [
        SeoVisibility::Draft,
        SeoVisibility::Held,
        SeoVisibility::Deleted,
        SeoVisibility::Scheduled,
        SeoVisibility::Unknown,
    ] {
        assert_eq!(
            open.decide(SeoPage::Chapter(SeoChapterFacts {
                visibility,
                ..chapter()
            }))
            .reason,
            SeoReason::ChapterNotPublic
        );
    }
    assert_eq!(
        open.decide(SeoPage::Chapter(SeoChapterFacts {
            has_public_version: false,
            ..chapter()
        }))
        .reason,
        SeoReason::ChapterHasNoPublicVersion
    );
    assert_eq!(
        open.decide(SeoPage::Chapter(SeoChapterFacts {
            chapter_id: 0,
            ..chapter()
        }))
        .reason,
        SeoReason::InvalidIdentity
    );
}

#[test]
fn f_08_seo_01_pilot_only_includes_explicitly_approved_books() {
    let pilot = policy(SeoSite::Pandaolate, SeoMode::Pilot);
    assert_eq!(
        pilot.decide(SeoPage::Home).reason,
        SeoReason::PilotNotApproved
    );
    assert_eq!(
        pilot
            .decide(SeoPage::Catalog(SeoCatalogView::Canonical))
            .reason,
        SeoReason::PilotNotApproved
    );
    assert!(!pilot
        .decide(SeoPage::Book {
            book: book(),
            view: SeoBookView::Canonical,
        })
        .indexable());
    let approved = SeoBookFacts {
        pilot_approved: true,
        ..book()
    };
    assert!(pilot
        .decide(SeoPage::Book {
            book: approved,
            view: SeoBookView::Canonical,
        })
        .indexable());
    assert!(pilot
        .decide(SeoPage::Chapter(SeoChapterFacts {
            book: approved,
            ..chapter()
        }))
        .indexable());
}

#[test]
fn f_08_seo_01_shadow_is_separate_from_book_access() {
    let panda = policy(SeoSite::Pandaolate, SeoMode::Public);
    let shadow = policy(SeoSite::Shadow, SeoMode::Public);
    let permitted_shadow = SeoBookFacts {
        shadow_enabled: true,
        ..book()
    };
    assert!(panda
        .decide(SeoPage::Book {
            book: permitted_shadow,
            view: SeoBookView::Canonical,
        })
        .indexable());
    assert_eq!(
        shadow
            .decide(SeoPage::Book {
                book: permitted_shadow,
                view: SeoBookView::Canonical,
            })
            .reason,
        SeoReason::DifferentIndexOwner
    );
    let shadow_owner = SeoBookFacts {
        index_owner: SeoSite::Shadow,
        ..book()
    };
    assert_eq!(
        shadow
            .decide(SeoPage::Book {
                book: shadow_owner,
                view: SeoBookView::Canonical,
            })
            .reason,
        SeoReason::ShadowDisabled
    );
    let shadow_owner = SeoBookFacts {
        shadow_enabled: true,
        ..shadow_owner
    };
    assert!(shadow
        .decide(SeoPage::Book {
            book: shadow_owner,
            view: SeoBookView::Canonical,
        })
        .indexable());
    let duplicate = panda.decide(SeoPage::Book {
        book: shadow_owner,
        view: SeoBookView::Canonical,
    });
    assert_eq!(duplicate.reason, SeoReason::DifferentIndexOwner);
    assert_eq!(duplicate.robots.meta(), "noindex,follow");
    assert!(!duplicate.sitemap_included);
    assert_eq!(
        shadow
            .decide(SeoPage::Chapter(SeoChapterFacts {
                book: shadow_owner,
                ..chapter()
            }))
            .canonical_path
            .as_deref(),
        Some("/reader/123")
    );
}

#[test]
fn f_08_seo_01_private_surfaces_remain_noindex_in_all_modes() {
    for site in [SeoSite::Pandaolate, SeoSite::Shadow] {
        for mode in [SeoMode::Disabled, SeoMode::Pilot, SeoMode::Public] {
            let seo = policy(site, mode);
            for page in [
                SeoPage::Private,
                SeoPage::Api,
                SeoPage::Service,
                SeoPage::Asset,
                SeoPage::SeoArtifact,
                SeoPage::Unknown,
            ] {
                let decision = seo.decide(page);
                assert_eq!(decision.robots, SeoRobots::NoIndexNoFollow);
                assert!(!decision.indexable());
                assert!(!decision.sitemap_included);
                assert_eq!(decision.canonical_path, None);
            }
        }
    }
}
