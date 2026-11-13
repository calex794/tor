//! Isolated negative route-policy tests for registered administrative and
//! intentionally unimplemented chapter moderation endpoints.
use super::*;

#[test]
fn rec_share_admin_has_only_private_bounded_get_and_post_routes() {
    let page = classify(Some("/admin/recommendations"), "GET");
    assert!(page.route_known);
    assert_eq!(page.traffic_surface, TrafficSurface::Admin);
    assert_eq!(page.product_area, ProductArea::Admin);
    assert_eq!(page.cost_class, CostClass::CheapRead);
    let save = classify(Some("/admin/recommendations"), "POST");
    assert!(save.route_known);
    assert_eq!(save.traffic_surface, TrafficSurface::Admin);
    assert_eq!(save.product_area, ProductArea::Admin);
    assert_eq!(save.cost_class, CostClass::Write);
    assert!(!classify(Some("/admin/recommendations"), "DELETE").route_known);
}

#[test]
fn per_chapter_moderation_has_no_registered_routes() {
    assert!(classify(Some("/moderation/books"), "GET").route_known);
    for (route, method) in [
        ("/moderation/chapters", "GET"),
        ("/moderation/chapters/{id}/hold", "POST"),
    ] {
        assert!(
            !classify(Some(route), method).route_known,
            "chapter-level moderation is not a product capability"
        );
    }
}

