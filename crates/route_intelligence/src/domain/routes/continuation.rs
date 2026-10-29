//! One continuation-demand passport; separated to keep the static registry bounded.
use super::super::*;
use CostClass::Write;
use ProductArea::Reading;
use RouteMatchKind::Prefix;
use TrafficSurface::PublicPage as Page;

pub(super) const REGISTRATION: RouteRegistration = RouteRegistration::new(
    "/read/{id}/want-continuation",
    "POST",
    "/read/",
    Prefix,
    Page,
    Reading,
    Write,
);
