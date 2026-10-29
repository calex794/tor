//! D-05 checkout route registration, separated to bound the taxonomy module.
use super::*;

pub(super) const LAYER_PURCHASE_ROUTE: RouteRegistration = RouteRegistration::new(
    "/api/v1/layers/{id}/purchase",
    "POST",
    "/api/v1/layers/",
    Prefix,
    Api,
    ProductArea::Billing,
    Write,
);
