use super::super::*;

#[test]
fn s349_free_grant_and_static_scripts_have_correct_costs() {
    let grant = classify(Some("/api/v1/chapters/{id}/free-grant"), "GET");
    assert!(grant.route_known);
    assert_eq!(grant.cost_class, CostClass::ExpensiveRead);
    assert_eq!(grant.product_area, ProductArea::Reading);
    assert!(!classify(Some("/api/v1/chapters/{id}/free-grant"), "POST").route_known);
    let pack = classify(Some("/api/v1/chapters/{id}/free-pack-source"), "GET");
    assert!(pack.route_known);
    assert_eq!(pack.cost_class, CostClass::ExpensiveRead);
    assert_eq!(pack.product_area, ProductArea::Reading);
    assert!(!classify(Some("/api/v1/chapters/{id}/free-pack-source"), "POST").route_known);
    for path in [
        "/assets/newboo-free-progress-s3.js",
        "/assets/newboo-free-s3-reader.js",
    ] {
        let asset = classify(Some(path), "GET");
        assert!(asset.route_known);
        assert_eq!(asset.traffic_surface, TrafficSurface::Static);
        assert_eq!(asset.cost_class, CostClass::Free);
        assert!(!classify(Some(path), "POST").route_known);
    }
}
