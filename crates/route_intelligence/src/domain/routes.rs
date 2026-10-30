//! Fixed MatchedPath taxonomy: immutable route IDs, no per-request allocation.
use super::*;

mod classifier;
mod continuation;
mod entries_core;
mod entries_follow;
mod entries_layer;
mod entries_reply;
pub use classifier::classify;

use entries_core::BASE_ROUTES;
use entries_follow::FOLLOW_ROUTES;
use entries_layer::{LAYER_INDEX_ROUTES, LAYER_METADATA_ROUTES, LAYER_ROUTES};
use entries_reply::REPLY_ROUTES;

const ROUTES: [RouteRegistration; 96] = {
    let mut result = [BASE_ROUTES[0]; 96];
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
    result[95] = LAYER_METADATA_ROUTES[0];
    result
};

/// Read-only, finite route graph for governance checks.
pub const fn registry() -> &'static [RouteRegistration] {
    &ROUTES
}
