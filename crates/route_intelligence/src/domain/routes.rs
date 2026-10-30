//! Fixed MatchedPath taxonomy: immutable route IDs, no per-request allocation.
use super::*;
use CostClass::{CheapRead as Read, Write};
use ProductArea::{Auth, Reading};
use RouteMatchKind::{Exact, Prefix};
use TrafficSurface::PublicPage as Page;

mod classifier;
mod continuation;
mod entries_core;
mod entries_follow;
mod entries_inbox;
mod entries_layer;
mod entries_reply;
pub use classifier::classify;

use entries_core::BASE_ROUTES;
use entries_follow::FOLLOW_ROUTES;
use entries_inbox::INBOX_ROUTES;
use entries_layer::{LAYER_INDEX_ROUTES, LAYER_METADATA_ROUTES, LAYER_ROUTES};
use entries_reply::REPLY_ROUTES;

const ROUTES: [RouteRegistration; 101] = {
    let mut result = [BASE_ROUTES[0]; 101];
    let mut i = 0;
    while i < 79 {
        result[i] = BASE_ROUTES[i];
        i += 1;
    }
    let mut j = 0;
    while j < 12 {
        result[79 + j] = LAYER_ROUTES[j];
        j += 1;
    }
    let mut k = 0;
    while k < 2 {
        result[91 + k] = FOLLOW_ROUTES[k];
        k += 1;
    }
    result[93] = REPLY_ROUTES[0];
    result[94] = LAYER_INDEX_ROUTES[0];
    result[95] = INBOX_ROUTES[0];
    result[96] = INBOX_ROUTES[1];
    result[97] = LAYER_METADATA_ROUTES[0];
    result[98] = RouteRegistration::new(
        "/me/updates/{id}/read-state",
        "POST",
        "/me/updates/",
        Prefix,
        Page,
        Reading,
        Write,
    );
    result[99] = RouteRegistration::new(
        "/me/settings/password",
        "GET",
        "/me/",
        Exact,
        Page,
        Auth,
        Read,
    );
    result[100] = RouteRegistration::new(
        "/me/settings/password",
        "POST",
        "/me/",
        Exact,
        Page,
        Auth,
        Write,
    );
    result
};

/// Read-only, finite route graph for governance checks.
pub const fn registry() -> &'static [RouteRegistration] {
    &ROUTES
}
