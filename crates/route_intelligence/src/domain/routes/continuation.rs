//! One continuation-demand passport; separated to keep the static registry bounded.
use super::*;

pub(super) const REGISTRATION: RouteRegistration = RouteRegistration::new(
    "/read/{id}/want-continuation",
    "POST",
    "/read/",
    Prefix,
    Page,
    Reading,
    Write,
);
